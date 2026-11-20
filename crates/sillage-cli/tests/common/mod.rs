//! Shared fixtures for the end-to-end tests: the paper's running example
//! (Listings 4-6) and small real-code corpora written to disk.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use sillage_core::{Lang, Params};
use sillage_engine::Engine;
use sillage_index::{AromaFeaturizer, Bm25};

/// Paper Listing 4 (query).
pub const LISTING4: &str = "\
if (view instanceof ViewGroup) {
    for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {
        View innerView = ((ViewGroup) view).getChildAt(i);
    }
}
";

/// Paper Listing 5 (arcbit `SendFragment`).
pub const LISTING5: &str = r#"package com.arcbit.ui;

public class SendFragment {
    public void setupUIToHideKeyBoardOnTouch(View view) {
        if (!(view instanceof EditText)) {
            view.setOnTouchListener(new View.OnTouchListener() {
                public boolean onTouch(View v, MotionEvent event) {
                    hideKeyBoard();
                    return false;
                }
            });
        }
        if (view instanceof ViewGroup) {
            for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {
                View innerView = ((ViewGroup) view).getChildAt(i);
                setupUIToHideKeyBoardOnTouch(innerView);
            }
        }
    }
}
"#;

/// Paper Listing 6 (`LoginActivity`).
pub const LISTING6: &str = r#"package com.example.login;

public class LoginActivity extends Activity {
    public void setupUI(View view) {
        if (!(view instanceof EditText)) {
            view.setOnTouchListener(new View.OnTouchListener() {
                public boolean onTouch(View v, MotionEvent event) {
                    Utils.toggleSoftKeyBoard(LoginActivity.this, true);
                    return false;
                }
            });
        }
        if (view instanceof ViewGroup) {
            for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {
                View innerView = ((ViewGroup) view).getChildAt(i);
                setupUI(innerView);
            }
        }
    }
}
"#;

/// Distractors: a near miss on the loop, an alternative loop shape, unrelated code.
pub const DISTRACTORS: &str = r#"package com.example.misc;

import java.util.List;

public class Misc {
    public int sum(List<Integer> xs) {
        int total = 0;
        for (int x : xs) {
            total += x;
        }
        return total;
    }

    public String join(String[] parts, String sep) {
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < parts.length; i++) {
            if (i > 0) {
                sb.append(sep);
            }
            sb.append(parts[i]);
        }
        return sb.toString();
    }

    public void disableAll(View root) {
        if (root instanceof ViewGroup) {
            for (int i = 0; i < ((ViewGroup) root).getChildCount(); i++) {
                ((ViewGroup) root).getChildAt(i).setEnabled(false);
            }
        }
        root.setEnabled(false);
    }

    public void logTree(View node, int depth) {
        Log.d("tree", depth + ":" + node);
        if (node instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) node;
            for (int i = 0; i < group.getChildCount(); i++) {
                logTree(group.getChildAt(i), depth + 1);
            }
        }
    }
}
"#;

/// A fresh directory under Cargo's per-target tmp dir (never `/tmp`).
pub fn scratch(name: &str) -> tempfile::TempDir {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new().prefix(name).tempdir_in(base).unwrap()
}

pub fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// Writes the Listing 5/6 + distractor corpus (one repo per file).
pub fn write_paper_corpus(root: &Path) {
    write(root, "arcbit/app/src/SendFragment.java", LISTING5);
    write(root, "login/app/src/LoginActivity.java", LISTING6);
    write(root, "misc/src/Misc.java", DISTRACTORS);
}

/// Real pipeline: walk → index → BM25 → save → load → engine.
pub fn engine_for(corpus_dir: &Path, langs: &[Lang], index_path: &Path) -> Engine {
    let (corpus, inv) = sillage_index::build_corpus_and_index(corpus_dir, langs, &AromaFeaturizer);
    assert!(!corpus.methods.is_empty(), "empty corpus under {}", corpus_dir.display());
    let bm25 = Bm25::build(&corpus);
    sillage_index::save(index_path, &corpus, &inv, &bm25).unwrap();
    let (corpus, inv, bm25) = sillage_index::load(index_path).unwrap();
    Engine::new(corpus, inv, bm25, Box::new(AromaFeaturizer), Params::default())
}

pub fn method_name(engine: &Engine, id: sillage_core::MethodId) -> String {
    engine.corpus.methods.iter().find(|m| m.id == id).map(|m| m.name.to_string()).unwrap()
}
