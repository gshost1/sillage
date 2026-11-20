use tree_sitter::Node;

use super::{
    CallRule, LangSpec, MethodRule, block, class, decl, decl_children, func, no_hook,
};

/// `Foo::bar`: the member name after `::` is never a variable.
fn method_ref_member(node: Node<'_>, parent: Node<'_>, _field: Option<&str>) -> bool {
    parent.kind() == "method_reference"
        && node
            .prev_sibling()
            .is_some_and(|p| p.kind() == "::")
}

pub static SPEC: LangSpec = LangSpec {
    atomic: &[
        "string_literal",
        "character_literal",
        "text_block",
        "decimal_integer_literal",
        "hex_integer_literal",
        "octal_integer_literal",
        "binary_integer_literal",
        "decimal_floating_point_literal",
        "hex_floating_point_literal",
    ],
    keyword_leaves: &["this", "super", "void_type", "boolean_type"],
    drop: &["line_comment", "block_comment"],
    calls: &[
        CallRule {
            kind: "method_invocation",
            args_field: "arguments",
            regroup_from: Some("name"),
        },
        CallRule {
            kind: "object_creation_expression",
            args_field: "arguments",
            regroup_from: None,
        },
        CallRule {
            kind: "explicit_constructor_invocation",
            args_field: "arguments",
            regroup_from: None,
        },
    ],
    args_kinds: &["argument_list"],
    ref_idents: &["identifier"],
    bind_idents: &["identifier"],
    scopes: &[
        block("block"),
        block("constructor_body"),
        block("switch_block"),
        block("switch_block_statement_group"),
        block("switch_rule"),
        block("for_statement"),
        block("enhanced_for_statement"),
        block("catch_clause"),
        block("try_with_resources_statement"),
        func("lambda_expression"),
        func("method_declaration"),
        func("constructor_declaration"),
        func("compact_constructor_declaration"),
        class("class_body"),
        class("interface_body"),
        class("enum_body"),
    ],
    decls: &[
        decl("formal_parameter", "name"),
        decl("variable_declarator", "name"),
        decl("catch_formal_parameter", "name"),
        decl("enhanced_for_statement", "name"),
        decl("lambda_expression", "parameters"),
        decl_children("inferred_parameters"),
        decl("resource", "name"),
        decl("instanceof_expression", "name"),
        decl_children("type_pattern"),
        decl_children("record_pattern_component"),
    ],
    containers: &[],
    pattern_skip: &[],
    non_var: &[
        ("method_invocation", "name"),
        ("field_access", "field"),
        ("method_declaration", "name"),
        ("constructor_declaration", "name"),
        ("compact_constructor_declaration", "name"),
        ("class_declaration", "name"),
        ("interface_declaration", "name"),
        ("enum_declaration", "name"),
        ("record_declaration", "name"),
        ("annotation_type_declaration", "name"),
        ("annotation_type_element_declaration", "name"),
        ("enum_constant", "name"),
        ("annotation", "name"),
        ("marker_annotation", "name"),
        ("element_value_pair", "key"),
        ("labeled_statement", "*"),
        ("break_statement", "*"),
        ("continue_statement", "*"),
        ("scoped_identifier", "*"),
        ("import_declaration", "*"),
        ("package_declaration", "*"),
        ("module_declaration", "*"),
        ("requires_module_directive", "*"),
        ("exports_module_directive", "*"),
        ("opens_module_directive", "*"),
        ("uses_module_directive", "*"),
        ("provides_module_directive", "*"),
    ],
    callee: &[],
    builtins: &["length"],
    upper_pattern_not_binding: false,
    non_var_hook: method_ref_member,
    free_global_hook: no_hook,
    methods: &[
        MethodRule {
            kind: "method_declaration",
            name_field: "name",
            body_field: "body",
        },
        MethodRule {
            kind: "constructor_declaration",
            name_field: "name",
            body_field: "body",
        },
        MethodRule {
            kind: "compact_constructor_declaration",
            name_field: "name",
            body_field: "body",
        },
    ],
    anon_fns: &[],
    named_parents: &[],
};
