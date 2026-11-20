# Slice A — `crates/sillage-parse`

Status: **done**. `cargo test -p sillage-parse` passes: 21 tests, 0 failures, 0 warnings.

## Public API
All the stub signatures are unchanged. One function was added: `pub fn ts_language(Lang) -> tree_sitter::Language`, which the grammar ABI test uses.

- `extract_methods` re-parses each method's own text with `parse_method`. That makes `ExtractedMethod::tree == parse_method(lang, text, body_span)` true by construction, and the fixture tests assert it. `text_span` indexes `src`. `body_span` is relative to `text_span.start`. `line_start` is 1-based.
- `parse_method` ignores `body` because the whole text is parsed. Free names stay globals.
- `parse_snippet` uses snippet mode (Decision 8). Spans index the snippet, and `tree.source == snippet`. An empty or whitespace-only snippet returns an empty tree.

## Modules
| File | Role |
|---|---|
| `lang/mod.rs` | `LangSpec` tables (declarative). They are compiled once per language into kind-id indexed `Tables`. |
| `lang/{java,python,javascript,typescript,go,rust}.rs` | Per-language tables. TS and TSX share one table. |
| `lower.rs` | CST → `SimplifiedTree`, plus a piecewise `Mapping` from wrapped text to original text. |
| `scope.rs` | Generic resolver that assigns `VarId`s to identifiers. |
| `snippet.rs` | Wrapper candidates. The first error-free parse wins; otherwise the parse with the fewest `ERROR`/`MISSING` nodes. |
| `extract.rs` | Method discovery. |

## Lowering rules
- Anonymous leaves become `Keyword`. Whitespace-only ones are dropped; Go emits `\n` statement terminators as tokens.
- Named leaves and atomic literals become `Token`. Comments, `MISSING` and zero-width nodes are dropped. `ERROR` nodes are lowered like any other node.
- A node with one child is replaced by that child. For a non-keyword child this is Def. 3. For a lone keyword child it is an extension, so Java `integral_type` lowers to the keyword `int`, which gives Fig. 4's `int#`.
- Call argument lists are spliced into the call node. A call then reads `#(#)` or `#()`, as in ANTLR.
- Java `obj.name(args)` is regrouped as `#.#` over `#(#)`, the Fig. 4 shape.
- Lexical keyword leaves become `Keyword`: Java `this`, `super`, `void` and `boolean`; Python `True`, `False` and `None`; Rust `mut`, `self`, `crate` and `super`.
- Depth guard at 1200 levels: anything deeper becomes one atomic token. Nodes with more than 30k children are chunked so `u16` slot indices still fit.

## Scoping
- Each scope node gets a declaration pre-pass from the `DeclRule` tables: parameters, locals, for/range/with/except/catch bindings, lambdas and closures, and destructuring patterns (container whitelist plus skipped fields).
- JS `var` hoists to the enclosing function. Python has function scopes with `global` and `nonlocal`. Class-scope names are never locals.
- In block-scoped languages a name is visible in its whole block, including before its declaration. This only differs from the real rules in code that would not compile, or in Go's `x := x` shadowing.
- Never a variable: `field_identifier`, `property_identifier`, `type_identifier`, member and method names, labels, import paths, keyword-argument names and the Java `::` member.
- Callee positions are a variable only when they resolve to a local declaration.
- Snippet mode: a free value identifier gets one `VarId` per name, unless it is a builtin, starts with a capital letter, or is a Go selector operand (package name).
- Known approximations:
  - Python module names are in the builtins list (`os`, `np`, …).
  - Nested function names are locals.
  - Rust `let` shadowing in the same scope reuses one `VarId`.

## Snippet wrappers (first error-free wins)
- **Java:** dummy class with dummy method (statements); then class body (members); then raw; then `return (<expr>);`.
- **Python:** raw; then `def __m():` with the snippet dedented and re-indented. The mapping is piecewise, per line.
- **JS/TS/TSX:** raw; then `function __m(){}`; then a class body; then `( … );`.
- **Go:** `package p; func __m(){}`; then raw; then `var __x = ( … )`.
- **Rust:** `fn __m(){}`; then raw; then `impl __S {}`.

Method-mode wrappers:
- **Java:** a class.
- **JS/TS:** class first, so `foo() {…}` methods do not parse as call-plus-block through ASI; then raw; then parenthesised.
- **Go and Rust:** raw first.

## Tests (`tests/`)
- **`grammars.rs`:** all 7 grammars pass the ABI range check, load and parse. Also covers `lang_from_path` extensions.
- **`fig4.rs`:** Listing 4 produces root `if##`. The labels `(#)`, `#instanceof#`, `{#}`, `#.#`, `(#)#`, `#()`, `#(#)`, `#<#`, `#++`, `#=#` and `int#;` are all present. The 3 `view` tokens share one `VarId`; `i` and `innerView` each have a distinct one; `ViewGroup`, `View`, `getChildCount` and `getChildAt` are `None`.
- **`fixtures.rs`:** rich real-code fixtures for all 7 languages, covering:
  - extraction and names, `validate()` and span text
  - `line_start` and `body_span`
  - `parse_method` round-trip equality
  - per-language scoping assertions
  - the whole file parsed as a snippet
- **`snippets.rs`:** Decision 8 locals versus callees, members and builtins in each language; method mode keeping names global; whole-method, expression and member snippets with no wrapper tokens leaking.
- **`robustness.rs`:**
  - the first 5 lines and every 5-line window of every multi-line method
  - proptest: random line windows, random byte ranges and arbitrary Unicode
  - odd inputs: empty input, unbalanced brackets, unterminated strings and comments, NUL bytes, 3000-deep parentheses, a 5000-term chain
  - every case must parse without panicking, pass `validate()` and have exact span text

## Environment notes
- The `/tmp` tmpfs hit the per-user quota during this slice: `/tmp/claude-1000` holds 3.7G, mostly other projects' Claude temp dirs. Nothing was deleted. The build used `CARGO_TARGET_DIR=target/slice-a`, which is on disk and gitignored, instead of `/tmp/sillage-target-slice-a`.
- `clippy` and `rustfmt` are not installed on this toolchain, so neither was run.
