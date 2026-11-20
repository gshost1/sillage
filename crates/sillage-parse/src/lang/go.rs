use tree_sitter::Node;

use super::{CallRule, LangSpec, MethodRule, Require, block, decl, func, no_hook};

/// `pkg.Func`: an unresolved selector operand is a package name, not a local.
fn package_operand(_node: Node<'_>, parent: Node<'_>, field: Option<&str>) -> bool {
    parent.kind() == "selector_expression" && field == Some("operand")
}

pub static SPEC: LangSpec = LangSpec {
    atomic: &[
        "interpreted_string_literal",
        "raw_string_literal",
        "rune_literal",
        "int_literal",
        "float_literal",
        "imaginary_literal",
    ],
    keyword_leaves: &[],
    drop: &["comment"],
    calls: &[CallRule {
        kind: "call_expression",
        args_field: "arguments",
        regroup_from: None,
    }],
    args_kinds: &["argument_list"],
    ref_idents: &["identifier"],
    bind_idents: &["identifier"],
    scopes: &[
        block("block"),
        block("for_statement"),
        block("if_statement"),
        block("expression_switch_statement"),
        block("type_switch_statement"),
        block("select_statement"),
        block("expression_case"),
        block("type_case"),
        block("communication_case"),
        block("default_case"),
        func("function_declaration"),
        func("method_declaration"),
        func("func_literal"),
    ],
    decls: &[
        decl("parameter_declaration", "name"),
        decl("variadic_parameter_declaration", "name"),
        decl("short_var_declaration", "left"),
        decl("var_spec", "name"),
        decl("const_spec", "name"),
        decl("range_clause", "left").require(Require::ChildToken(":=")),
        decl("receive_statement", "left").require(Require::ChildToken(":=")),
        decl("type_switch_statement", "alias"),
    ],
    containers: &["expression_list"],
    pattern_skip: &[],
    non_var: &[
        ("function_declaration", "name"),
        ("import_spec", "*"),
        ("package_clause", "*"),
    ],
    callee: &[("call_expression", "function")],
    builtins: &[
        "append", "cap", "clear", "close", "complex", "copy", "delete", "imag", "len", "make",
        "max", "min", "new", "panic", "print", "println", "real", "recover", "nil", "true",
        "false", "iota", "bool", "byte", "complex64", "complex128", "error", "float32",
        "float64", "int", "int8", "int16", "int32", "int64", "rune", "string", "uint", "uint8",
        "uint16", "uint32", "uint64", "uintptr", "any",
    ],
    upper_pattern_not_binding: false,
    non_var_hook: no_hook,
    free_global_hook: package_operand,
    methods: &[
        MethodRule {
            kind: "function_declaration",
            name_field: "name",
            body_field: "body",
        },
        MethodRule {
            kind: "method_declaration",
            name_field: "name",
            body_field: "body",
        },
    ],
    anon_fns: &[],
    named_parents: &[],
};
