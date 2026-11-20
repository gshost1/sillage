use super::{
    CallRule, DeclRule, Hoist, LangSpec, MethodRule, Require, ScopeRule, block, class, decl,
    decl_children, func, no_hook,
};

pub const ATOMIC: &[&str] = &["string", "template_string", "regex", "number"];
pub const DROP: &[&str] = &["comment", "html_comment", "hash_bang_line"];
pub const CALLS: &[CallRule] = &[
    CallRule {
        kind: "call_expression",
        args_field: "arguments",
        regroup_from: None,
    },
    CallRule {
        kind: "new_expression",
        args_field: "arguments",
        regroup_from: None,
    },
];
pub const REF_IDENTS: &[&str] = &["identifier", "shorthand_property_identifier"];
pub const BIND_IDENTS: &[&str] = &["identifier", "shorthand_property_identifier_pattern"];
pub const SCOPES: &[ScopeRule] = &[
    block("statement_block"),
    block("for_statement"),
    block("for_in_statement"),
    block("catch_clause"),
    block("switch_body"),
    func("function_declaration"),
    func("generator_function_declaration"),
    func("function_expression"),
    func("function"),
    func("generator_function"),
    func("arrow_function"),
    func("method_definition"),
    class("class_body"),
];
pub const DECLS: &[DeclRule] = &[
    decl("variable_declarator", "name").hoist(Hoist::FunctionIfParent("variable_declaration")),
    decl_children("formal_parameters"),
    decl("arrow_function", "parameter"),
    decl("catch_clause", "parameter"),
    decl("for_in_statement", "left").require(Require::Field("kind")),
    decl("function_declaration", "name").hoist(Hoist::Enclosing),
    decl("generator_function_declaration", "name").hoist(Hoist::Enclosing),
];
pub const CONTAINERS: &[&str] = &[
    "object_pattern",
    "array_pattern",
    "pair_pattern",
    "assignment_pattern",
    "object_assignment_pattern",
    "rest_pattern",
    "required_parameter",
    "optional_parameter",
];
pub const PATTERN_SKIP: &[(&str, &str)] = &[
    ("pair_pattern", "key"),
    ("assignment_pattern", "right"),
    ("object_assignment_pattern", "right"),
    ("required_parameter", "type"),
    ("required_parameter", "value"),
    ("required_parameter", "decorator"),
    ("optional_parameter", "type"),
    ("optional_parameter", "value"),
    ("optional_parameter", "decorator"),
];
pub const NON_VAR: &[(&str, &str)] = &[
    ("function_expression", "name"),
    ("function", "name"),
    ("generator_function", "name"),
    ("class", "name"),
    ("class_declaration", "name"),
    ("import_specifier", "*"),
    ("import_clause", "*"),
    ("namespace_import", "*"),
    ("export_specifier", "*"),
    ("namespace_export", "*"),
    ("jsx_opening_element", "name"),
    ("jsx_closing_element", "name"),
    ("jsx_self_closing_element", "name"),
    ("nested_identifier", "*"),
    ("member_expression", "property"),
    // TypeScript
    ("enum_declaration", "name"),
    ("module", "name"),
    ("internal_module", "name"),
    ("ambient_declaration", "*"),
];
pub const CALLEE: &[(&str, &str)] = &[
    ("call_expression", "function"),
    ("new_expression", "constructor"),
];
pub const BUILTINS: &[&str] = &[
    "console", "window", "document", "globalThis", "global", "require", "module", "exports",
    "process", "undefined", "NaN", "Infinity", "arguments", "parseInt", "parseFloat", "isNaN",
    "isFinite", "setTimeout", "setInterval", "clearTimeout", "clearInterval", "fetch", "alert",
    "encodeURIComponent", "decodeURIComponent", "encodeURI", "decodeURI", "__dirname",
    "__filename", "navigator", "location", "localStorage", "sessionStorage", "self",
];
pub const METHODS: &[MethodRule] = &[
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
];
pub const ANON_FNS: &[&str] = &[
    "arrow_function",
    "function_expression",
    "function",
    "generator_function",
];
pub const NAMED_PARENTS: &[(&str, &str)] = &[
    ("variable_declarator", "name"),
    ("pair", "key"),
    ("assignment_expression", "left"),
    ("field_definition", "property"),
    ("public_field_definition", "name"),
];

pub static SPEC: LangSpec = LangSpec {
    atomic: ATOMIC,
    keyword_leaves: &["this", "super"],
    drop: DROP,
    calls: CALLS,
    args_kinds: &["arguments"],
    ref_idents: REF_IDENTS,
    bind_idents: BIND_IDENTS,
    scopes: SCOPES,
    decls: DECLS,
    containers: CONTAINERS,
    pattern_skip: PATTERN_SKIP,
    non_var: NON_VAR,
    callee: CALLEE,
    builtins: BUILTINS,
    upper_pattern_not_binding: false,
    non_var_hook: no_hook,
    free_global_hook: no_hook,
    methods: METHODS,
    anon_fns: ANON_FNS,
    named_parents: NAMED_PARENTS,
};
