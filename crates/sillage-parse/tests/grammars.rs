//! Grammar ABI check: every language loads into a tree-sitter parser.

use sillage_core::Lang;

#[test]
fn all_grammars_load() {
    assert_eq!(Lang::ALL.len(), 7);
    for &lang in Lang::ALL {
        let language = sillage_parse::ts_language(lang);
        let abi = language.abi_version();
        assert!(
            (tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION..=tree_sitter::LANGUAGE_VERSION)
                .contains(&abi),
            "{lang}: incompatible ABI {abi}"
        );
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&language)
            .unwrap_or_else(|e| panic!("{lang}: {e}"));
        let tree = parser.parse("x", None).expect("parse");
        assert!(tree.root_node().end_byte() >= 1, "{lang}");
    }
}

#[test]
fn lang_from_path_maps_extensions() {
    use sillage_parse::lang_from_path;
    use std::path::Path;
    let cases = [
        ("A.java", Some(Lang::Java)),
        ("a.py", Some(Lang::Python)),
        ("a.pyi", Some(Lang::Python)),
        ("a.js", Some(Lang::JavaScript)),
        ("a.jsx", Some(Lang::JavaScript)),
        ("a.mjs", Some(Lang::JavaScript)),
        ("a.cjs", Some(Lang::JavaScript)),
        ("a.ts", Some(Lang::TypeScript)),
        ("a.mts", Some(Lang::TypeScript)),
        ("a.cts", Some(Lang::TypeScript)),
        ("a.tsx", Some(Lang::Tsx)),
        ("dir/a.go", Some(Lang::Go)),
        ("src/lib.rs", Some(Lang::Rust)),
        ("A.JAVA", Some(Lang::Java)),
        ("a.c", None),
        ("Makefile", None),
    ];
    for (p, want) in cases {
        assert_eq!(lang_from_path(Path::new(p)), want, "{p}");
    }
}
