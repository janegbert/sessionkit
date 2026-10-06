// jevgrep's four Python helpers (inspect, neighborhood, preview, calls), on tree-sitter instead of
// CPython 3.11. What they read from the syntax tree is small: def and class nodes with their name,
// first line (including decorators) and last line, the order of class bodies, the first body
// statement, self.x() calls, and base-class text.
//
// jevgrep treats a file that CPython 3.11 cannot parse as text. tree-sitter recovers from errors
// instead, so a tree with an error, or with syntax newer than 3.11 or older than Python 3, counts
// as a parse failure here. Compile-time errors that CPython's parser reports and tree-sitter does
// not (such as `del 1`) still parse; CPython's extra line breaks at a lone CR are not counted.

use super::source::{Declared, Range};
use std::collections::{HashMap, HashSet};
use tree_sitter::{Node, Parser, Tree};
use unicode_normalization::UnicodeNormalization;

pub struct Parsed<'a> {
    tree: Tree,
    source: &'a str,
    /// Byte offset where each LF-separated row starts.
    rows: Vec<usize>,
}

/// Parse as CPython 3.11 would accept it, or None.
pub fn parse(source: &str) -> Option<Parsed<'_>> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_python::LANGUAGE.into()).ok()?;
    let tree = parser.parse(source, None)?;
    let mut rows = vec![0];
    rows.extend(source.match_indices('\n').map(|(at, _)| at + 1));
    let parsed = Parsed { tree, source, rows };
    if rejected(parsed.tree.root_node(), source) { None } else { Some(parsed) }
}

/// Syntax CPython 3.11 rejects: errors, Python 2 statements, PEP 695 and PEP 701 syntax, and
/// what CPython's tokenizer and parser refuse where tree-sitter recovers silently.
fn rejected(root: Node, source: &str) -> bool {
    indentation_error(source).is_some() || rejection(root, source).is_some()
}

/// CPython's tokenizer rules for indentation: each dedent must return to an earlier level, and
/// tabs must give the same levels with tab size 8 and tab size 1 (else TabError). Only the start
/// of a logical line counts: not inside brackets, strings or after a backslash.
pub fn indentation_error(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
    let (mut depth, mut i, mut line) = (0usize, 0usize, 1usize);
    let mut at_line_start = true;
    while i < bytes.len() {
        if at_line_start && depth == 0 {
            let (mut column, mut alternate, mut j) = (0, 0, i);
            while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\x0c') {
                match bytes[j] {
                    b' ' => {
                        column += 1;
                        alternate += 1;
                    }
                    b'\t' => {
                        column = (column / 8 + 1) * 8;
                        alternate += 1;
                    }
                    _ => {
                        column = 0;
                        alternate = 0;
                    }
                }
                j += 1;
            }
            at_line_start = false;
            let blank = j >= bytes.len() || matches!(bytes[j], b'\n' | b'\r' | b'#');
            if !blank {
                let &(top, top_alternate) = stack.last().unwrap();
                if column > top {
                    if alternate <= top_alternate {
                        return Some(line);
                    }
                    stack.push((column, alternate));
                } else {
                    while stack.last().is_some_and(|&(level, _)| column < level) {
                        stack.pop();
                    }
                    match stack.last() {
                        Some(&(level, level_alternate)) if level == column && level_alternate == alternate => {}
                        _ => return Some(line),
                    }
                }
            }
            i = j;
            continue;
        }
        match bytes[i] {
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'\\' => {
                // A backslash continues the line; the next line is not a new logical line.
                if bytes.get(i + 1) == Some(&b'\n') {
                    line += 1;
                    i += 2;
                    continue;
                }
                i += 2;
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'\'' | b'"' => {
                let quote = bytes[i];
                let triple = bytes.get(i + 1) == Some(&quote) && bytes.get(i + 2) == Some(&quote);
                // An empty string '' is not the start of a triple-quoted one.
                let empty = !triple && bytes.get(i + 1) == Some(&quote);
                i += if triple { 3 } else if empty { 2 } else { 1 };
                if empty {
                    continue;
                }
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => {
                            if bytes.get(i + 1) == Some(&b'\n') {
                                line += 1;
                            }
                            i += 2;
                            continue;
                        }
                        b'\n' => {
                            line += 1;
                            if !triple {
                                break;
                            }
                        }
                        c if c == quote && (!triple || (bytes.get(i + 1) == Some(&quote) && bytes.get(i + 2) == Some(&quote))) => {
                            i += if triple { 3 } else { 1 };
                            break;
                        }
                        _ => {}
                    }
                    i += 1;
                }
                continue;
            }
            b'\n' => {
                line += 1;
                // Inside brackets a line break does not start a logical line.
                at_line_start = depth == 0;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Why a tree counts as a parse failure, with the line.
pub fn rejection(root: Node, source: &str) -> Option<String> {
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let line = node.start_position().row + 1;
        if node.is_error() || node.is_missing() {
            return Some(format!("{} at line {line}-{}", if node.is_error() { "error" } else { "missing" }, node.end_position().row + 1));
        }
        match node.kind() {
            kind @ ("print_statement" | "exec_statement") => return Some(format!("{kind} at line {line}")),
            // tree-sitter also reads `type(x).y = z` as a type alias; only `type Name =` or
            // `type Name[` is one.
            "type_alias_statement" if is_type_alias(&source[node.byte_range()]) => return Some(format!("type alias at line {line}")),
            "function_definition" | "class_definition" if node.child_by_field_name("type_parameters").is_some() => {
                return Some(format!("type parameters at line {line}"));
            }
            "string" if newer_string(node, source) => return Some(format!("string at line {line}")),
            // An empty suite, which tree-sitter reads as an empty block.
            "block" if block_is_empty(node) => return Some(format!("empty block at line {line}")),
            "parameters" | "lambda_parameters" if default_before_plain(node) => return Some(format!("parameter order at line {line}")),
            "argument_list" if argument_order_wrong(node) => return Some(format!("argument order at line {line}")),
            // `except A, B:` without parentheses: Python 2, or Python 3.14.
            "except_clause" | "except_group_clause" if unparenthesized_except(node) => return Some(format!("except list at line {line}")),
            _ => {}
        }
        let children: Vec<Node> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    None
}

/// Test builds: the syntax tree as an S-expression.
#[cfg(feature = "test-api")]
pub fn tree_of(source: &str) -> String {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_python::LANGUAGE.into()).unwrap();
    parser.parse(source, None).unwrap().root_node().to_sexp()
}

/// Test builds: why a source does not parse, if it does not.
#[cfg(feature = "test-api")]
pub fn rejection_of(source: &str) -> Option<String> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_python::LANGUAGE.into()).ok()?;
    let tree = parser.parse(source, None)?;
    indentation_error(source).map(|line| format!("indentation at line {line}")).or_else(|| rejection(tree.root_node(), source))
}

fn block_is_empty(node: Node) -> bool {
    let mut cursor = node.walk();
    !node.named_children(&mut cursor).any(|child| !child.is_extra())
}

/// `def f(a=1, b)`: a parameter without a default after one with a default, before any `*`.
fn default_before_plain(node: Node) -> bool {
    let mut cursor = node.walk();
    let mut seen_default = false;
    for parameter in node.named_children(&mut cursor).filter(|p| !p.is_extra()) {
        match parameter.kind() {
            "default_parameter" | "typed_default_parameter" => seen_default = true,
            "identifier" | "typed_parameter" if seen_default => {
                // `*args: T` is a typed_parameter holding a list_splat_pattern.
                let splat = parameter.named_child(0).is_some_and(|c| c.kind().ends_with("splat_pattern"));
                if !splat {
                    return true;
                }
                return false;
            }
            "list_splat_pattern" | "dictionary_splat_pattern" | "keyword_separator" => return false,
            _ => {}
        }
    }
    false
}

/// `f(**a, *b)` or `f(a=1, b)`: an iterable unpacking after `**`, or a positional argument after
/// a keyword argument or `**`.
fn argument_order_wrong(node: Node) -> bool {
    let mut cursor = node.walk();
    let (mut keyword, mut double_splat) = (false, false);
    for argument in node.named_children(&mut cursor) {
        if argument.is_extra() {
            continue;
        }
        match argument.kind() {
            "keyword_argument" => keyword = true,
            "dictionary_splat" => double_splat = true,
            "list_splat" if double_splat => return true,
            "list_splat" => {}
            // A generator expression argument is its own node, not a positional after others.
            _ if keyword || double_splat => return true,
            _ => {}
        }
    }
    false
}

fn has_comment(node: Node) -> bool {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    children.into_iter().any(|child| child.kind() == "comment" || has_comment(child))
}

fn unparenthesized_except(node: Node) -> bool {
    let mut cursor = node.walk();
    node.children_by_field_name("value", &mut cursor).count() > 1
}

fn is_type_alias(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("type") else { return false };
    let rest = rest.trim_start();
    let name_length = rest.find(|c: char| !(c == '_' || c.is_alphanumeric())).unwrap_or(rest.len());
    name_length > 0 && rest.len() < text.len() - 4 && rest[name_length..].trim_start().starts_with(['=', '['])
}

/// A string only newer Pythons accept: a t-string (3.14), or an f-string whose expression reuses
/// the enclosing quote, holds a backslash or a comment, or spans lines in a single-quoted string
/// (3.12).
fn newer_string(node: Node, source: &str) -> bool {
    let text = &source[node.byte_range()];
    let quote_at = text.find(['\'', '"']).unwrap_or(0);
    let prefix = text[..quote_at].to_ascii_lowercase();
    if prefix.contains('t') {
        return true;
    }
    if !prefix.contains('f') {
        return false;
    }
    let quote = if text[quote_at..].starts_with("\"\"\"") || text[quote_at..].starts_with("'''") {
        &text[quote_at..quote_at + 3]
    } else {
        &text[quote_at..quote_at + 1]
    };
    let mut cursor = node.walk();
    node.children(&mut cursor).filter(|child| child.kind() == "interpolation").any(|interpolation| {
        let inner = interpolation.child_by_field_name("expression").map_or("", |e| &source[e.byte_range()]);
        inner.contains(quote.chars().next().unwrap()) && quote.len() == 1
            || inner.contains(quote) && quote.len() == 3
            || inner.contains('\\')
            || has_comment(interpolation)
            || quote.len() == 1 && source[interpolation.byte_range()].contains('\n')
    })
}

impl<'a> Parsed<'a> {
    fn row_of(&self, byte: usize) -> usize {
        self.rows.partition_point(|&start| start <= byte)
    }

    fn text(&self, node: Node) -> &'a str {
        &self.source[node.byte_range()]
    }

    /// CPython's lineno: the line where the statement's own node starts. A decorated def starts
    /// at `def` (or `async`), not at its decorators.
    fn lineno(&self, statement: Node) -> usize {
        statement.start_position().row + 1
    }

    /// CPython's end_lineno: the line of the last token that is not a comment.
    fn end_lineno(&self, node: Node) -> usize {
        let mut current = node;
        loop {
            let mut cursor = current.walk();
            let children: Vec<Node> = current.children(&mut cursor).collect();
            match children.into_iter().rev().find(|child| child.kind() != "comment" && child.end_byte() > child.start_byte()) {
                Some(child) => current = child,
                None => return self.row_of(current.end_byte().saturating_sub(1).max(current.start_byte())),
            }
        }
    }

    /// The node an AST statement corresponds to: a decorated definition is its def or class.
    fn definition<'t>(node: Node<'t>) -> Option<(Node<'t>, Option<Node<'t>>)> {
        match node.kind() {
            "function_definition" | "class_definition" => Some((node, None)),
            "decorated_definition" => node.child_by_field_name("definition").map(|definition| (definition, Some(node))),
            _ => None,
        }
    }

    /// The first line of a def or class, with its decorators: the line of each decorator's
    /// expression, as CPython reports it (inside parentheses, not the `@` line).
    fn start(&self, definition: Node, decorated: Option<Node>) -> usize {
        let mut start = self.lineno(definition);
        if let Some(decorated) = decorated {
            let mut cursor = decorated.walk();
            for decorator in decorated.children(&mut cursor).filter(|child| child.kind() == "decorator") {
                let mut expression = decorator.named_child(0);
                while let Some(inner) = expression.filter(|e| e.kind() == "parenthesized_expression") {
                    expression = inner.named_child(0).filter(|child| child.kind() != "comment").or(Some(inner));
                    if expression == Some(inner) {
                        break;
                    }
                }
                if let Some(expression) = expression {
                    start = start.min(self.lineno(expression));
                }
            }
        }
        start
    }

    fn name(&self, definition: Node) -> String {
        definition.child_by_field_name("name").map_or(String::new(), |name| self.text(name).nfkc().collect())
    }

    /// The statements of a def or class body, without comments.
    fn body<'t>(definition: Node<'t>) -> Vec<Node<'t>> {
        let Some(block) = definition.child_by_field_name("body") else { return Vec::new() };
        let mut cursor = block.walk();
        block.named_children(&mut cursor).filter(|child| child.kind() != "comment").collect()
    }

    fn definitions_in<'t>(statements: &[Node<'t>]) -> Vec<(Node<'t>, Option<Node<'t>>)> {
        statements.iter().filter_map(|statement| Self::definition(*statement)).collect()
    }

    fn statements(&self) -> Vec<Node<'_>> {
        let root = self.tree.root_node();
        let mut cursor = root.walk();
        root.named_children(&mut cursor).filter(|child| child.kind() != "comment").collect()
    }

    /// All nodes, breadth first, as ast.walk visits them.
    fn walk(&self) -> Vec<Node<'_>> {
        let root = self.tree.root_node();
        let mut queue = std::collections::VecDeque::from([root]);
        let mut all = Vec::new();
        while let Some(node) = queue.pop_front() {
            all.push(node);
            let mut cursor = node.walk();
            queue.extend(node.named_children(&mut cursor));
        }
        all
    }
}

// ---------------------------------------------------------------- inspect.py

/// Declaration boundaries: top-level defs and classes, and the members of classes, with the
/// parts of a class between its members as Name.context. None when the source does not parse.
pub fn declarations(source: &str) -> Option<Vec<Declared>> {
    let parsed = parse(source)?;
    let mut units = Vec::new();
    let statements = parsed.statements();
    visit(&parsed, &Parsed::definitions_in(&statements), "", &[], &mut units);
    Some(units)
}

fn visit<'t>(parsed: &'t Parsed, nodes: &[(Node<'t>, Option<Node<'t>>)], prefix: &str, owner_headers: &[Range], units: &mut Vec<Declared>) {
    for &(definition, decorated) in nodes {
        let start = parsed.start(definition, decorated);
        let end = parsed.end_lineno(definition);
        let name = format!("{prefix}{}", parsed.name(definition));
        let children = Parsed::definitions_in(&Parsed::body(definition));
        if definition.kind() == "class_definition" && !children.is_empty() {
            let first_start = parsed.start(children[0].0, children[0].1);
            let mut headers = owner_headers.to_vec();
            if first_start > start {
                headers.push(Range::new(start, first_start - 1));
            }
            let mut cursor = start;
            for &child in &children {
                let child_start = parsed.start(child.0, child.1);
                if cursor < child_start {
                    units.push(Declared { name: format!("{name}.context"), range: Range::new(cursor, child_start - 1), owner_headers: headers.clone() });
                }
                visit(parsed, &[child], &format!("{name}."), &headers, units);
                cursor = parsed.end_lineno(child.0) + 1;
            }
            if cursor <= end {
                units.push(Declared { name: format!("{name}.context"), range: Range::new(cursor, end), owner_headers: headers.clone() });
            }
        } else {
            units.push(Declared { name, range: Range::new(start, end), owner_headers: owner_headers.to_vec() });
        }
    }
}

// ---------------------------------------------------------------- neighborhood.py

/// Structural context around selected methods: the class header (at most 40 lines) and the
/// methods just before and after, when they are at most 40 lines. Empty when it does not parse.
pub fn neighborhood(source: &str, ranges: &[Range]) -> Vec<Range> {
    let Some(parsed) = parse(source) else { return Vec::new() };
    let mut extra = Vec::new();
    let mut add = |first: usize, last: usize| {
        if first <= last {
            extra.push(Range::new(first, last));
        }
    };
    for node in parsed.walk().into_iter().filter(|node| node.kind() == "function_definition") {
        let decorated = node.parent().filter(|parent| parent.kind() == "decorated_definition");
        let Some(owner) = owner_class(decorated.unwrap_or(node)) else { continue };
        let start = parsed.start(node, decorated);
        let end = parsed.end_lineno(node);
        if !ranges.iter().any(|r| r.start <= end && r.end >= start) {
            continue;
        }
        let owner_decorated = owner.parent().filter(|parent| parent.kind() == "decorated_definition");
        let owner_start = parsed.start(owner, owner_decorated);
        let siblings = Parsed::definitions_in(&Parsed::body(owner));
        let header_end = siblings.iter().map(|s| parsed.start(s.0, s.1) - 1).min().unwrap_or_else(|| parsed.end_lineno(owner));
        add(owner_start, header_end.min(owner_start + 39));
        let Some(index) = siblings.iter().position(|s| s.0 == node) else { continue };
        for &(neighbor, neighbor_decorated) in &siblings[index.saturating_sub(1)..(index + 2).min(siblings.len())] {
            let neighbor_start = parsed.start(neighbor, neighbor_decorated);
            let neighbor_end = parsed.end_lineno(neighbor);
            if neighbor != node && neighbor_end + 1 - neighbor_start <= 40 {
                add(neighbor_start, neighbor_end);
            }
        }
    }
    extra
}

/// The class whose body holds this statement directly.
fn owner_class(statement: Node) -> Option<Node> {
    let block = statement.parent().filter(|parent| parent.kind() == "block")?;
    block.parent().filter(|parent| parent.kind() == "class_definition")
}

// ---------------------------------------------------------------- preview.py

pub struct PreviewSpan {
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
    pub byte_start: usize,
    pub byte_end: usize,
    pub partial_line: bool,
    pub column: Option<(usize, usize)>,
}

pub struct Preview {
    pub text: String,
    pub spans: Vec<PreviewSpan>,
    pub truncated: bool,
    pub preview_bytes: usize,
}

struct Match {
    start: usize,
    end: usize,
    context: Option<(usize, usize)>,
    header_end: usize,
    header_line: usize,
    header_column_end: usize,
    body_start: usize,
    body_column: usize,
}

/// Python's str.isidentifier.
fn is_identifier(token: &str) -> bool {
    let mut chars = token.chars();
    chars.next().is_some_and(|c| c == '_' || unicode_ident::is_xid_start(c)) && chars.all(unicode_ident::is_xid_continue)
}

/// The identifiers in a query, after NFKC: the names a preview looks for.
fn query_tokens(query: &str) -> HashSet<String> {
    let mut tokens = HashSet::new();
    let mut token = String::new();
    for c in query.nfkc().chain(std::iter::once(' ')) {
        if is_identifier(&format!("_{c}")) {
            token.push(c);
        } else {
            if is_identifier(&token) {
                tokens.insert(token.clone());
            }
            token.clear();
        }
    }
    tokens
}

/// bytes.decode('utf-8', errors='ignore').
fn decode_ignoring(bytes: &[u8]) -> String {
    bytes.utf8_chunks().map(|chunk| chunk.valid()).collect()
}

/// An Expr statement whose value is a str constant: a docstring.
fn is_docstring(statement: Node, source: &str) -> bool {
    if statement.kind() != "expression_statement" || statement.named_child_count() != 1 {
        return false;
    }
    let mut value = statement.named_child(0);
    while let Some(inner) = value.filter(|v| v.kind() == "parenthesized_expression") {
        value = inner.named_child(0);
    }
    let Some(value) = value else { return false };
    let is_str = |string: Node| {
        let text = &source[string.byte_range()];
        let prefix = &text[..text.find(['\'', '"']).unwrap_or(0)].to_ascii_lowercase();
        !prefix.contains('f') && !prefix.contains('b')
    };
    match value.kind() {
        "string" => is_str(value),
        "concatenated_string" => {
            let mut cursor = value.walk();
            value.named_children(&mut cursor).all(is_str)
        }
        _ => false,
    }
}

/// Query-assisted content windows for a file above the budget: the opening, the declarations the
/// query names, and three spread-out windows. Never a file-admission filter.
pub fn preview(query: &str, text: &str, budget: usize) -> Preview {
    let lines: Vec<&str> = text.split('\n').collect();
    let source_bytes = text.len();
    let mut line_offsets = Vec::with_capacity(lines.len());
    let mut offset = 0;
    for line in &lines {
        line_offsets.push(offset);
        offset += line.len() + 1;
    }
    if source_bytes <= budget {
        return Preview {
            text: text.to_string(),
            spans: vec![PreviewSpan { start_line: 1, end_line: lines.len(), text: text.to_string(), byte_start: 0, byte_end: source_bytes, partial_line: false, column: None }],
            truncated: false,
            preview_bytes: source_bytes,
        };
    }
    let tokens = query_tokens(query);
    let mut matches: Vec<Match> = Vec::new();
    if let Some(parsed) = parse(text) {
        for node in parsed.walk() {
            if !matches!(node.kind(), "function_definition" | "class_definition") || !tokens.contains(&parsed.name(node)) {
                continue;
            }
            let decorated = node.parent().filter(|parent| parent.kind() == "decorated_definition");
            let start = parsed.start(node, decorated);
            let mut ancestor = decorated.unwrap_or(node).parent();
            let mut context = None;
            while let Some(owner) = ancestor {
                if owner.kind() == "class_definition" {
                    let owner_decorated = owner.parent().filter(|parent| parent.kind() == "decorated_definition");
                    let first = Parsed::body(owner)[0];
                    context = Some((parsed.start(owner, owner_decorated), anchor_lineno(&parsed, first) - 1));
                    break;
                }
                ancestor = owner.parent();
            }
            let body = Parsed::body(node);
            let Some(&first) = body.first() else { continue };
            let (mut chosen, mut body_column) = (first, 0);
            if is_docstring(first, text) && body.len() > 1 {
                chosen = body[1];
                if anchor_lineno(&parsed, chosen) == anchor_lineno(&parsed, first) {
                    body_column = anchor(chosen).start_position().column;
                }
            }
            let header_line = parsed.lineno(node);
            matches.push(Match {
                start,
                end: parsed.end_lineno(node),
                context,
                header_end: anchor_lineno(&parsed, first) - 1,
                header_line,
                header_column_end: if anchor_lineno(&parsed, first) == header_line { anchor(first).start_position().column } else { 0 },
                body_start: anchor_lineno(&parsed, chosen),
                body_column,
            });
        }
    }
    matches.sort_by_key(|m| (m.start, m.end));

    let mut windows = Windows { lines: &lines, line_offsets: &line_offsets, budget, used: 0, spans: Vec::new(), parts: Vec::new(), seen: HashSet::new() };
    windows.add(1, lines.len(), budget / 4, "opening context", false);
    if !matches.is_empty() {
        let per_match = 1.max(((budget - windows.used) as f64 * 0.75) as usize / matches.len());
        for m in &matches {
            let mut context_cost = 0;
            if let Some((first, last)) = m.context {
                let before = windows.used;
                windows.add(first, last, (per_match / 3).min(512), "enclosing class context", false);
                context_cost = windows.used - before;
            }
            let before = windows.used;
            let header_allowance = (per_match.saturating_sub(context_cost) / 3).min(512);
            windows.add(m.start, m.header_end, header_allowance, "query-named declaration header", false);
            if m.header_column_end > 0 {
                windows.add_inline(m.header_line, 0, m.header_column_end, header_allowance, "query-named declaration header");
            }
            let header_cost = windows.used - before;
            let remaining = per_match as i64 - context_cost as i64 - header_cost as i64;
            if m.body_column > 0 {
                let before = windows.used;
                let line_length = lines[m.body_start - 1].len();
                windows.add_inline(m.body_start, m.body_column, line_length, remaining.max(0) as usize, "query-named implementation");
                if m.end > m.body_start {
                    let rest = remaining - (windows.used - before) as i64;
                    windows.add(m.body_start + 1, m.end, rest.max(0) as usize, "query-named implementation continuation", false);
                }
            } else {
                windows.add(m.body_start, m.end, remaining.max(0) as usize, "query-named implementation", false);
            }
        }
    }
    // Non-matching files still have real content previews; the query never excludes them.
    let count = lines.len();
    let mut positions: Vec<usize> = vec![count / 3 + 1, 2 * count / 3 + 1, 1.max(count.saturating_sub(31))];
    positions.sort();
    positions.dedup();
    let total = positions.len();
    for (i, &start) in positions.iter().enumerate() {
        let allowance = (budget - windows.used) / (total - i);
        windows.add(start, (start + 31).min(count), allowance, "distributed context", i == total - 1);
    }
    Preview { text: windows.parts.concat(), spans: windows.spans, truncated: true, preview_bytes: windows.used }
}

/// The node that carries a statement's lineno and col_offset: the def or class of a decorated one.
fn anchor(statement: Node) -> Node {
    if statement.kind() == "decorated_definition" { statement.child_by_field_name("definition").unwrap_or(statement) } else { statement }
}

fn anchor_lineno(parsed: &Parsed, statement: Node) -> usize {
    parsed.lineno(anchor(statement))
}

struct Windows<'a> {
    lines: &'a [&'a str],
    line_offsets: &'a [usize],
    budget: usize,
    used: usize,
    spans: Vec<PreviewSpan>,
    parts: Vec<String>,
    seen: HashSet<(usize, usize, usize, String)>,
}

impl Windows<'_> {
    fn add(&mut self, start: usize, end: usize, allowance: usize, basis: &str, from_end: bool) -> bool {
        if start > end {
            return false;
        }
        let mut start = start.max(1);
        let end = end.min(self.lines.len());
        let allowance = allowance.min(self.budget - self.used);
        let header = format!("--- source lines {start}-{end}; {basis}; may be clipped ---\n");
        let remaining = allowance as i64 - header.len() as i64 - 1;
        if remaining <= 0 {
            return false;
        }
        let remaining = remaining as usize;
        let mut selected: Vec<String> = Vec::new();
        let (mut size, mut partial) = (0, false);
        let source_lines = &self.lines[(start - 1).min(self.lines.len())..end.max(start - 1)];
        let ordered: Vec<&&str> = if from_end { source_lines.iter().rev().collect() } else { source_lines.iter().collect() };
        for line in ordered {
            let cost = line.len() + usize::from(!selected.is_empty());
            if size + cost > remaining {
                if selected.is_empty() {
                    let raw = line.as_bytes();
                    let cut = if from_end { &raw[raw.len() - remaining.min(raw.len())..] } else { &raw[..remaining.min(raw.len())] };
                    let text = decode_ignoring(cut);
                    if !text.is_empty() {
                        selected = vec![text];
                        partial = true;
                    }
                }
                break;
            }
            selected.push(line.to_string());
            size += cost;
        }
        if selected.is_empty() {
            return false;
        }
        if from_end {
            selected.reverse();
            start = end + 1 - selected.len();
        }
        let actual_end = start + selected.len() - 1;
        let body = selected.join("\n");
        let identity = (start, actual_end, 0, body.clone());
        if self.seen.contains(&identity) {
            return true;
        }
        // The actual range replaces the requested range; clipping is explicit.
        let label = format!("--- source lines {start}-{actual_end}; {basis}{} ---\n", if partial { "; partial line" } else { "" });
        let rendered = format!("{label}{body}\n");
        if rendered.len() > allowance {
            return false;
        }
        let byte_start = self.line_offsets[start - 1] + if partial && from_end { self.lines[start - 1].len() - body.len() } else { 0 };
        self.seen.insert(identity);
        self.spans.push(PreviewSpan { start_line: start, end_line: actual_end, byte_end: byte_start + body.len(), text: body, byte_start, partial_line: partial, column: None });
        self.used += rendered.len();
        self.parts.push(rendered);
        true
    }

    fn add_inline(&mut self, line_number: usize, start_byte: usize, end_byte: usize, allowance: usize, basis: &str) -> bool {
        let raw = self.lines[line_number - 1].as_bytes();
        let end_byte = end_byte.min(raw.len());
        let allowance = allowance.min(self.budget - self.used);
        let longest = format!("--- source line {line_number}, bytes {start_byte}-{end_byte}; {basis}; partial line ---\n");
        let room = allowance as i64 - longest.len() as i64 - 1;
        if room <= 0 {
            return false;
        }
        let slice = &raw[start_byte.min(end_byte)..end_byte];
        let body = decode_ignoring(&slice[..(room as usize).min(slice.len())]);
        if body.is_empty() {
            return false;
        }
        let actual_end = start_byte + body.len();
        let label = format!("--- source line {line_number}, bytes {start_byte}-{actual_end}; {basis}; partial line ---\n");
        let rendered = format!("{label}{body}\n");
        let identity = (line_number, start_byte, actual_end, body.clone());
        if self.seen.contains(&identity) {
            return true;
        }
        self.seen.insert(identity);
        self.used += rendered.len();
        self.parts.push(rendered);
        let offset = self.line_offsets[line_number - 1];
        self.spans.push(PreviewSpan {
            start_line: line_number, end_line: line_number, byte_start: offset + start_byte, byte_end: offset + actual_end,
            text: body, partial_line: true, column: Some((start_byte, actual_end)),
        });
        true
    }
}

/// The checks jevgrep applies to a truncated preview before it trusts it (core/source.ts
/// pythonPreview): every span must match the source it names.
pub fn checked_preview(query: &str, source: &str, budget: usize) -> Option<Preview> {
    let result = preview(query, source, budget);
    if !result.truncated {
        return Some(result);
    }
    let raw = source.as_bytes();
    let lines: Vec<&str> = source.split('\n').collect();
    let bad = result.spans.is_empty() || result.spans.iter().any(|span| {
        if span.start_line < 1 || span.end_line < span.start_line || span.end_line > lines.len() {
            return true;
        }
        if span.byte_end < span.byte_start || span.byte_end > raw.len() || String::from_utf8_lossy(&raw[span.byte_start..span.byte_end]) != span.text {
            return true;
        }
        let original = lines[span.start_line - 1..span.end_line].join("\n");
        if let Some((start, end)) = span.column {
            let bytes = original.as_bytes();
            return span.start_line != span.end_line || end < start || end > bytes.len() || String::from_utf8_lossy(&bytes[start..end]) != span.text;
        }
        if span.partial_line { !original.contains(&span.text) } else { original != span.text }
    });
    if bad { None } else { Some(result) }
}

// ---------------------------------------------------------------- calls.py

pub struct Call {
    pub caller: String,
    pub name: String,
    pub range: Range,
    pub unknown_earlier_bases: Vec<String>,
    pub owner_header: Range,
}

/// ast.unparse of a simple base expression: its text without line breaks and extra spaces.
fn unparse(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(collapsed.len());
    let chars: Vec<char> = collapsed.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == ' ' && (i > 0 && matches!(chars[i - 1], '(' | '[' | '.') || chars.get(i + 1).is_some_and(|n| matches!(n, ')' | ']' | ',' | '.'))) {
            continue;
        }
        out.push(c);
    }
    out
}

/// Inherited same-file self-method calls in the selected lines: reading leads, not proofs of
/// runtime dispatch. None when the source does not parse or has an inconsistent MRO.
pub fn calls(source: &str, ranges: &[Range]) -> Option<Vec<Call>> {
    if source.replace("\r\n", "").contains('\r') {
        return Some(Vec::new());
    }
    let parsed = parse(source)?;
    let lines = split_lines(source);
    let declared: Vec<(Node, Option<Node>)> = parsed.statements().into_iter().filter_map(Parsed::definition)
        .filter(|(definition, _)| definition.kind() == "class_definition").collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (class, _) in &declared {
        *counts.entry(parsed.name(*class)).or_default() += 1;
    }
    let classes: indexmap::IndexMap<String, Node> = declared.iter().filter(|(class, _)| counts[&parsed.name(*class)] == 1)
        .map(|(class, _)| (parsed.name(*class), *class)).collect();
    let mut methods: indexmap::IndexMap<String, indexmap::IndexMap<String, (Node, Option<Node>)>> = indexmap::IndexMap::new();
    for (name, class) in &classes {
        let definitions: Vec<(Node, Option<Node>)> = Parsed::definitions_in(&Parsed::body(*class)).into_iter()
            .filter(|(definition, _)| definition.kind() == "function_definition").collect();
        let mut duplicates: HashMap<String, usize> = HashMap::new();
        for (definition, _) in &definitions {
            *duplicates.entry(parsed.name(*definition)).or_default() += 1;
        }
        methods.insert(name.clone(), definitions.into_iter().filter(|(d, _)| duplicates[&parsed.name(*d)] == 1).map(|d| (parsed.name(d.0), d)).collect());
    }
    let bases: HashMap<String, Vec<String>> = classes.iter().map(|(name, class)| {
        let list = class.child_by_field_name("superclasses").map(|arguments| {
            let mut cursor = arguments.walk();
            arguments.named_children(&mut cursor).filter(|child| !matches!(child.kind(), "keyword_argument" | "comment"))
                .map(|child| unparse(parsed.text(child))).collect()
        }).unwrap_or_default();
        (name.clone(), list)
    }).collect();

    let mut linearizations: HashMap<String, Vec<String>> = HashMap::new();
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for (owner, definitions) in &methods {
        for (name, &(function, decorated)) in definitions {
            if decorated.is_some() || first_parameter(&parsed, function).as_deref() != Some("self") {
                continue;
            }
            let nodes = body_nodes(function);
            if nodes.iter().any(|node| assigns_self(&parsed, *node)) {
                continue;
            }
            for node in nodes {
                let Some(attribute) = self_call(&parsed, node) else { continue };
                let line = parsed.lineno(node);
                if !ranges.iter().any(|r| r.start <= line && line <= r.end) {
                    continue;
                }
                let mut unknown = Vec::new();
                for ancestor in mro(owner, &bases, &mut linearizations, &[])? {
                    if !classes.contains_key(&ancestor) {
                        unknown.push(ancestor);
                        continue;
                    }
                    let Some(&(target, target_decorated)) = methods[&ancestor].get(&attribute) else { continue };
                    if ancestor == *owner {
                        break; // Same-owner helpers remain under relevance selection.
                    }
                    let target_end = parsed.end_lineno(target);
                    if ranges.iter().any(|r| r.start <= parsed.lineno(target) && r.end >= target_end) {
                        break;
                    }
                    if !seen.insert((owner.clone(), name.clone(), ancestor.clone(), attribute.clone())) {
                        break;
                    }
                    let mut start = parsed.start(target, target_decorated);
                    while start > 1 && lines.get(start - 2).is_some_and(|line| line.trim_start().starts_with('#')) {
                        start -= 1;
                    }
                    let class = classes[&ancestor];
                    let first = Parsed::body(class)[0];
                    result.push(Call {
                        caller: format!("{owner}.{name}"),
                        name: format!("{ancestor}.{attribute}"),
                        range: Range::new(start, target_end),
                        unknown_earlier_bases: unknown,
                        owner_header: Range::new(parsed.lineno(class), anchor_lineno(&parsed, first) - 1),
                    });
                    break;
                }
            }
        }
    }
    Some(result)
}

/// Python's str.splitlines(): LF, CR, CRLF, and also VT, FF, FS, GS, RS, NEL, U+2028 and U+2029
/// end a line.
fn split_lines(source: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = source.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            lines.push(&source[start..at]);
            let mut end = at + c.len_utf8();
            if c == '\r' && chars.peek().is_some_and(|&(_, next)| next == '\n') {
                chars.next();
                end += 1;
            }
            start = end;
        }
    }
    if start < source.len() {
        lines.push(&source[start..]);
    }
    lines
}

/// The name of the first positional parameter.
fn first_parameter(parsed: &Parsed, function: Node) -> Option<String> {
    let parameters = function.child_by_field_name("parameters")?;
    let mut cursor = parameters.walk();
    let first = parameters.named_children(&mut cursor).find(|p| p.kind() != "comment" && p.kind() != "positional_separator")?;
    match first.kind() {
        "identifier" => Some(parsed.text(first).to_string()),
        "typed_parameter" => first.named_child(0).filter(|n| n.kind() == "identifier").map(|n| parsed.text(n).to_string()),
        "default_parameter" | "typed_default_parameter" => first.child_by_field_name("name").filter(|n| n.kind() == "identifier").map(|n| parsed.text(n).to_string()),
        _ => None,
    }
}

/// The nodes of a function, without nested functions, classes and lambdas.
fn body_nodes(function: Node) -> Vec<Node> {
    let mut nodes = Vec::new();
    let mut stack = vec![function];
    while let Some(node) = stack.pop() {
        nodes.push(node);
        let mut cursor = node.walk();
        let children: Vec<Node> = node.named_children(&mut cursor)
            .filter(|child| !matches!(child.kind(), "function_definition" | "class_definition" | "decorated_definition" | "lambda"))
            .collect();
        stack.extend(children.into_iter().rev());
    }
    nodes
}

/// `self.name(...)`: the name, when this node is such a call.
fn self_call(parsed: &Parsed, node: Node) -> Option<String> {
    if node.kind() != "call" {
        return None;
    }
    let function = node.child_by_field_name("function").filter(|f| f.kind() == "attribute")?;
    let object = function.child_by_field_name("object").filter(|o| o.kind() == "identifier" && parsed.text(*o) == "self")?;
    let _ = object;
    function.child_by_field_name("attribute").map(|attribute| parsed.text(attribute).nfkc().collect())
}

/// Whether this node stores to the name self: an assignment, loop, walrus or `as` target.
fn assigns_self(parsed: &Parsed, node: Node) -> bool {
    let target = match node.kind() {
        "assignment" | "augmented_assignment" | "for_statement" | "for_in_clause" => node.child_by_field_name("left"),
        "named_expression" => node.child_by_field_name("name"),
        "as_pattern" => node.child_by_field_name("alias"),
        _ => None,
    };
    target.is_some_and(|target| stores_self(parsed, target))
}

fn stores_self(parsed: &Parsed, target: Node) -> bool {
    match target.kind() {
        "identifier" => parsed.text(target) == "self",
        "attribute" | "subscript" => false,
        _ => {
            let mut cursor = target.walk();
            let children: Vec<Node> = target.named_children(&mut cursor).collect();
            children.into_iter().any(|child| stores_self(parsed, child))
        }
    }
}

/// The C3 linearization of a class over the names in this file; None on a cycle or conflict.
fn mro(name: &str, bases: &HashMap<String, Vec<String>>, cache: &mut HashMap<String, Vec<String>>, seen: &[String]) -> Option<Vec<String>> {
    if seen.iter().any(|s| s == name) {
        return None;
    }
    if let Some(done) = cache.get(name) {
        return Some(done.clone());
    }
    let parents = bases.get(name).cloned().unwrap_or_default();
    let mut path = seen.to_vec();
    path.push(name.to_string());
    let mut sequences: Vec<Vec<String>> = Vec::new();
    for parent in &parents {
        sequences.push(mro(parent, bases, cache, &path)?);
    }
    sequences.push(parents.clone());
    let mut result = vec![name.to_string()];
    while sequences.iter().any(|s| !s.is_empty()) {
        sequences.retain(|s| !s.is_empty());
        let head = sequences.iter().map(|s| s[0].clone()).find(|head| !sequences.iter().any(|t| t[1..].contains(head)))?;
        result.push(head.clone());
        for sequence in &mut sequences {
            if sequence[0] == head {
                sequence.remove(0);
            }
        }
    }
    cache.insert(name.to_string(), result.clone());
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_follow_python_line_splitting() {
        // A form feed ends a line for str.splitlines, which shifts the comment walk-back.
        let source = "class Base:\n    x = 1\n    \x0c\n    # about helper\n    def helper(self):\n        return 1\nclass T(Base):\n    def run(self):\n        return self.helper()\n";
        let calls = calls(source, &[Range::new(8, 9)]).unwrap();
        assert_eq!((calls[0].range.start, calls[0].range.end), (5, 6));
    }

    #[test]
    fn indentation_follows_cpython() {
        assert!(indentation_error("def f():\n    x = 1\n\ty = 2\n").is_some());
        assert!(indentation_error("if x:\n        a = 1\n    b = 2\n").is_some());
        assert!(indentation_error("f(a,\n  b)\nif x:\n    y = [1,\n2]\n").is_none());
        assert!(indentation_error("s = '''\n  x\n\ty'''\n").is_none());
    }
}
