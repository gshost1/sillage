//! TypeScript and TSX share one table; the grammars differ only in JSX support.

use super::javascript as js;
use super::{LangSpec, MethodRule, no_hook};

pub static SPEC: LangSpec = LangSpec {
    atomic: js::ATOMIC,
    keyword_leaves: &["this", "super", "this_type"],
    drop: js::DROP,
    calls: js::CALLS,
    args_kinds: &["arguments"],
    ref_idents: js::REF_IDENTS,
    bind_idents: js::BIND_IDENTS,
    scopes: js::SCOPES,
    decls: js::DECLS,
    containers: js::CONTAINERS,
    pattern_skip: js::PATTERN_SKIP,
    non_var: js::NON_VAR,
    callee: js::CALLEE,
    builtins: js::BUILTINS,
    upper_pattern_not_binding: false,
    non_var_hook: no_hook,
    free_global_hook: no_hook,
    methods: &[
        MethodRule {
            kind: "function_declaration",
            name_field: "name",
            body_field: "body",
        },
        MethodRule {
            kind: "generator_function_declaration",
            name_field: "name",
            body_field: "body",
        },
        MethodRule {
            kind: "method_definition",
            name_field: "name",
            body_field: "body",
        },
    ],
    anon_fns: js::ANON_FNS,
    named_parents: js::NAMED_PARENTS,
};
