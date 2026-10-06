// TypeScript and JavaScript units, on oxc instead of the TypeScript compiler (jevgrep's
// core/source.ts). A unit is every top-level statement; a class declaration with members becomes
// a header unit (Name.context) and one unit per member. A unit's name is what TypeScript's
// `node.name.getText()` gives, the declarator names of a variable statement, or "source". Lines
// follow TypeScript's line map, which also breaks at a lone CR, U+2028 and U+2029.
//
// Comments: TypeScript reports the comments in the leading trivia of each AST node and the
// trailing comments on the same line after a node. A comment on its own line before a closing
// brace belongs to neither, so jevgrep does not see it; that rule is kept here.
//
// A file with any parse error falls back to text, as a file with TypeScript parse diagnostics
// does in jevgrep. oxc reports a few errors that TypeScript leaves to its type checker.

use super::source::{Declared, Range};
use oxc_allocator::Allocator;
use oxc_ast::AstKind;
use oxc_ast::ast::{
    Class, ClassElement, Declaration, ExportDefaultDeclarationKind, MethodDefinitionKind, ModuleDeclaration, PropertyKey, Statement,
};
use oxc_ast_visit::Visit;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::{GetSpan, SourceType};
use std::collections::HashSet;

pub struct Parsed {
    pub units: Vec<Declared>,
    pub comments: Vec<Range>,
    pub syntax_error: bool,
}

fn source_type(path: &str) -> SourceType {
    let Ok(source_type) = SourceType::from_path(path) else { return SourceType::ts() };
    // TypeScript parses JSX in JavaScript files, and a .js file may be a script or a module.
    if source_type.is_javascript() { source_type.with_jsx(true).with_unambiguous(true) } else { source_type }
}

/// TypeScript's line starts, in bytes: CR, LF, CRLF, U+2028 and U+2029 end a line.
fn line_starts(source: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    let mut starts = vec![0];
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                i += if bytes.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
                starts.push(i);
            }
            b'\n' => {
                i += 1;
                starts.push(i);
            }
            0xe2 if bytes.get(i + 1) == Some(&0x80) && matches!(bytes.get(i + 2), Some(0xa8 | 0xa9)) => {
                i += 3;
                starts.push(i);
            }
            _ => i += 1,
        }
    }
    starts
}

struct Lines {
    starts: Vec<usize>,
}

impl Lines {
    fn line(&self, byte: usize) -> usize {
        self.starts.partition_point(|&start| start <= byte)
    }
}

struct Context<'s> {
    source: &'s str,
    lines: Lines,
    units: Vec<Declared>,
}

impl Context<'_> {
    fn text(&self, start: u32, end: u32) -> &str {
        &self.source[start as usize..end as usize]
    }

    /// A unit from its first byte (decorators and modifiers included) to its last.
    fn unit(&mut self, name: String, start: u32, end: u32, owner_headers: &[Range]) {
        let first = self.lines.line(start as usize);
        let last = self.lines.line((start as usize).max((end as usize).saturating_sub(1)));
        self.units.push(Declared { name, range: Range::new(first, last), owner_headers: owner_headers.to_vec() });
    }

    /// A class declaration with members: its header and each member; without members, one unit.
    fn class(&mut self, class: &Class, name: String, start: u32, end: u32) {
        let members: Vec<(&ClassElement, u32)> = class.body.body.iter().map(|member| (member, member_start(member))).collect();
        if members.is_empty() {
            return self.unit(name, start, end, &[]);
        }
        let first = self.lines.line(start as usize);
        let first_member = self.lines.line(members[0].1 as usize);
        let mut headers = Vec::new();
        if first_member > first {
            let header = Range::new(first, first_member - 1);
            headers.push(header);
            self.units.push(Declared { name: format!("{name}.context"), range: header, owner_headers: headers.clone() });
        }
        for (member, member_start) in members {
            let member_name = match member {
                ClassElement::MethodDefinition(method) if method.kind == MethodDefinitionKind::Constructor => "source".to_string(),
                ClassElement::MethodDefinition(method) => self.key(&method.key, method.computed),
                ClassElement::PropertyDefinition(property) => self.key(&property.key, property.computed),
                ClassElement::AccessorProperty(property) => self.key(&property.key, property.computed),
                ClassElement::StaticBlock(_) | ClassElement::TSIndexSignature(_) => "source".to_string(),
            };
            self.unit(format!("{name}.{member_name}"), member_start, member.span().end, &headers);
        }
    }

    /// The text of a member name; a computed name keeps its brackets, as TypeScript's does.
    fn key(&self, key: &PropertyKey, computed: bool) -> String {
        let span = key.span();
        if !computed {
            return self.text(span.start, span.end).to_string();
        }
        let before = &self.source[..span.start as usize];
        let open = before.rfind('[').unwrap_or(span.start as usize);
        let after = &self.source[span.end as usize..];
        let close = after.find(']').map_or(span.end as usize, |at| span.end as usize + at + 1);
        self.source[open..close].to_string()
    }

    fn declaration(&mut self, declaration: &Declaration, start: u32, end: u32) {
        let name = match declaration {
            Declaration::VariableDeclaration(variables) => {
                let names: Vec<&str> = variables.declarations.iter().map(|d| self.text(d.id.span().start, d.id.span().end)).collect();
                names.join(", ")
            }
            Declaration::FunctionDeclaration(function) => function.id.as_ref().map_or("source".into(), |id| id.name.to_string()),
            Declaration::ClassDeclaration(class) => {
                let name = class.id.as_ref().map_or("source".into(), |id| id.name.to_string());
                return self.class(class, name, start.min(decorators_start(class)), end);
            }
            Declaration::TSTypeAliasDeclaration(alias) => alias.id.name.to_string(),
            Declaration::TSInterfaceDeclaration(interface) => interface.id.name.to_string(),
            Declaration::TSEnumDeclaration(enumeration) => enumeration.id.name.to_string(),
            Declaration::TSExternalModuleDeclaration(module) => self.text(module.id.span.start, module.id.span.end).to_string(),
            Declaration::TSNamespaceDeclaration(namespace) => namespace.id.name.to_string(),
            Declaration::TSGlobalDeclaration(_) => "global".into(),
            Declaration::TSImportEqualsDeclaration(import) => import.id.name.to_string(),
        };
        self.unit(name, start, end, &[]);
    }

    fn statement(&mut self, statement: &Statement) {
        let span = statement.span();
        let (start, end) = (span.start, span.end);
        if let Some(declaration) = statement.as_declaration() {
            return self.declaration(declaration, start, end);
        }
        if let Some(module) = statement.as_module_declaration() {
            match module {
                ModuleDeclaration::ExportDeclaration(export) => return self.declaration(&export.declaration, start, end),
                ModuleDeclaration::ExportDefaultDeclaration(export) => match &export.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                        let name = function.id.as_ref().map_or("source".into(), |id| id.name.to_string());
                        return self.unit(name, start, end, &[]);
                    }
                    ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                        let name = class.id.as_ref().map_or("source".into(), |id| id.name.to_string());
                        return self.class(class, name, start.min(decorators_start(class)), end);
                    }
                    ExportDefaultDeclarationKind::TSInterfaceDeclaration(interface) => {
                        return self.unit(interface.id.name.to_string(), start, end, &[]);
                    }
                    _ => {}
                },
                ModuleDeclaration::TSNamespaceExportDeclaration(export) => {
                    let name = self.text(export.id.span.start, export.id.span.end).to_string();
                    return self.unit(name, start, end, &[]);
                }
                _ => {}
            }
        }
        self.unit("source".into(), start, end, &[]);
    }
}

fn decorators_start(class: &Class) -> u32 {
    class.decorators.first().map_or(u32::MAX, |decorator| decorator.span.start)
}

/// Where a class member starts: its first decorator, else its first modifier.
fn member_start(member: &ClassElement) -> u32 {
    let decorators = match member {
        ClassElement::MethodDefinition(method) => method.decorators.first(),
        ClassElement::PropertyDefinition(property) => property.decorators.first(),
        ClassElement::AccessorProperty(property) => property.decorators.first(),
        _ => None,
    };
    decorators.map_or(member.span().start, |decorator| decorator.span.start.min(member.span().start))
}

/// Node kinds that are not nodes in TypeScript's own tree: lists, bodies and annotations.
fn not_a_typescript_node(kind: &AstKind) -> bool {
    matches!(kind, AstKind::Program(_) | AstKind::Hashbang(_) | AstKind::FormalParameters(_) | AstKind::ClassBody(_)
        | AstKind::TSTypeAnnotation(_) | AstKind::TSTypeParameterDeclaration(_) | AstKind::TSTypeParameterInstantiation(_)
        | AstKind::TSInterfaceBody(_) | AstKind::TSEnumBody(_))
}

/// Where nodes start and end. Some tokens are nodes in TypeScript's tree too: the operator of a
/// binary expression, `?` and `:` of a conditional, and `=>` of an arrow function.
struct Bounds<'s> {
    source: &'s str,
    comments: &'s [(u32, u32)],
    starts: HashSet<u32>,
    ends: HashSet<u32>,
}

impl Bounds<'_> {
    /// The first position at or after this one that is not whitespace or a comment.
    fn skip_trivia(&self, mut at: usize) -> usize {
        loop {
            let rest = &self.source[at.min(self.source.len())..];
            at += rest.len() - rest.trim_start().len();
            match self.comments.binary_search_by_key(&(at as u32), |&(start, _)| start) {
                Ok(index) => at = self.comments[index].1 as usize,
                Err(_) => return at,
            }
        }
    }

    /// Modifier keywords at the start of a declaration, which are nodes in TypeScript's tree.
    fn modifiers_from(&mut self, position: u32) {
        const MODIFIERS: [&str; 12] = ["export", "default", "declare", "async", "static", "abstract", "public", "private", "protected", "readonly", "override", "accessor"];
        let mut at = self.skip_trivia(position as usize);
        loop {
            let rest = &self.source[at.min(self.source.len())..];
            let word_length = rest.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')).unwrap_or(rest.len());
            if word_length == 0 || !MODIFIERS.contains(&&rest[..word_length]) {
                return;
            }
            self.starts.insert(at as u32);
            self.ends.insert((at + word_length) as u32);
            at = self.skip_trivia(at + word_length);
        }
    }

    /// A token that TypeScript keeps as a node, found after the given position.
    fn token_after(&mut self, position: u32, token: &str) {
        let at = self.skip_trivia(position as usize);
        if self.source[at.min(self.source.len())..].starts_with(token) {
            self.starts.insert(at as u32);
            self.ends.insert((at + token.len()) as u32);
        }
    }
}

impl<'a> Visit<'a> for Bounds<'_> {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        if !not_a_typescript_node(&kind) {
            let span = kind.span();
            self.starts.insert(span.start);
            self.ends.insert(span.end);
        }
        match kind {
            AstKind::BinaryExpression(e) => self.token_after(e.left.span().end, e.operator.as_str()),
            AstKind::LogicalExpression(e) => self.token_after(e.left.span().end, e.operator.as_str()),
            AstKind::AssignmentExpression(e) => self.token_after(e.left.span().end, e.operator.as_str()),
            AstKind::PrivateInExpression(e) => self.token_after(e.left.span.end, "in"),
            AstKind::ConditionalExpression(e) => {
                self.token_after(e.test.span().end, "?");
                self.token_after(e.consequent.span().end, ":");
            }
            AstKind::ArrowFunctionExpression(e) => {
                let before = e.return_type.as_ref().map_or(e.params.span.end, |annotation| annotation.span.end);
                self.token_after(before, "=>");
                self.modifiers_from(e.span.start);
            }
            // TypeScript reads `a, b` as a binary expression whose comma is a node.
            AstKind::SequenceExpression(e) => {
                for expression in e.expressions.iter().take(e.expressions.len().saturating_sub(1)) {
                    self.token_after(expression.span().end, ",");
                }
            }
            AstKind::ExportNamedDeclaration(e) => self.modifiers_from(e.span.start),
            AstKind::ExportDeclaration(e) => self.modifiers_from(e.span.start),
            AstKind::ExportDefaultDeclaration(e) => self.modifiers_from(e.span.start),
            AstKind::Function(e) => self.modifiers_from(e.span.start),
            AstKind::Class(e) => self.modifiers_from(e.decorators.last().map_or(e.span.start, |d| d.span.end)),
            AstKind::MethodDefinition(e) => self.modifiers_from(e.decorators.last().map_or(e.span.start, |d| d.span.end)),
            AstKind::PropertyDefinition(e) => self.modifiers_from(e.decorators.last().map_or(e.span.start, |d| d.span.end)),
            AstKind::AccessorProperty(e) => self.modifiers_from(e.decorators.last().map_or(e.span.start, |d| d.span.end)),
            AstKind::VariableDeclaration(e) => self.modifiers_from(e.span.start),
            AstKind::TSInterfaceDeclaration(e) => self.modifiers_from(e.span.start),
            AstKind::TSTypeAliasDeclaration(e) => self.modifiers_from(e.span.start),
            AstKind::TSEnumDeclaration(e) => self.modifiers_from(e.span.start),
            _ => {}
        }
    }
}

fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// The comments TypeScript reports: in the leading trivia of a node or of the end of the file,
/// or trailing a node on the same line.
fn comments(spans: &[(u32, u32)], bounds: &Bounds, source: &str, lines: &Lines) -> Vec<Range> {
    let mut ranges = Vec::new();
    for (i, &(start, end)) in spans.iter().enumerate() {
        // The first code after this comment, skipping whitespace and later comments.
        let mut at = end as usize;
        let mut next_comment = i + 1;
        loop {
            let rest = &source[at..];
            at += rest.len() - rest.trim_start().len();
            if next_comment < spans.len() && spans[next_comment].0 as usize == at {
                at = spans[next_comment].1 as usize;
                next_comment += 1;
                continue;
            }
            break;
        }
        // TypeScript collects leading comments only after a line break, or at the start of the file.
        let mut back = start as usize;
        let mut previous_comment = i;
        let after_break = loop {
            let kept = source[..back].trim_end_matches(|c: char| c.is_whitespace() && !is_line_break(c));
            if kept.ends_with(is_line_break) {
                break true;
            }
            back = kept.len();
            if back == 0 {
                break true;
            }
            if previous_comment > 0 && spans[previous_comment - 1].1 as usize == back {
                previous_comment -= 1;
                back = spans[previous_comment].0 as usize;
                continue;
            }
            break false;
        };
        let leading = after_break && (at >= source.len() || bounds.starts.contains(&(at as u32)));
        // The last code before this comment on the same line, skipping earlier comments.
        let mut back = start as usize;
        let mut previous_comment = i;
        let trailing = loop {
            let kept = source[..back].trim_end_matches(|c: char| c.is_whitespace() && !is_line_break(c));
            back = kept.len();
            if back == 0 || kept.ends_with(is_line_break) {
                break false;
            }
            if previous_comment > 0 && spans[previous_comment - 1].1 as usize == back {
                previous_comment -= 1;
                back = spans[previous_comment].0 as usize;
                continue;
            }
            break bounds.ends.contains(&(back as u32));
        };
        if leading || trailing {
            ranges.push(Range::new(lines.line(start as usize), lines.line(end as usize - 1)));
        }
    }
    ranges
}

fn options() -> ParseOptions {
    ParseOptions { allow_return_outside_function: true, ..ParseOptions::default() }
}

pub fn parse(path: &str, source: &str) -> Parsed {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type(path)).with_options(options()).parse();
    let syntax_error = parsed.fatal_error || parsed.diagnostics.has_errors();
    let program = &parsed.program;
    let mut context = Context { source, lines: Lines { starts: line_starts(source) }, units: Vec::new() };
    if !syntax_error {
        for directive in &program.directives {
            context.unit("source".into(), directive.span.start, directive.span.end, &[]);
        }
        for statement in &program.body {
            context.statement(statement);
        }
    }
    let spans: Vec<(u32, u32)> = program.comments.iter().map(|comment| (comment.span.start, comment.span.end)).collect();
    let mut bounds = Bounds { source, comments: &spans, starts: HashSet::new(), ends: HashSet::new() };
    bounds.visit_program(program);
    let comments = comments(&spans, &bounds, source, &context.lines);
    Parsed { units: context.units, comments, syntax_error }
}

/// Test builds: the first parse error, with its line.
#[cfg(feature = "test-api")]
pub fn error_of(path: &str, source: &str) -> Option<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type(path)).with_options(options()).parse();
    parsed.diagnostics.errors().next().map(|error| {
        let at = error.labels.first().map_or(0, |label| label.offset() as usize);
        let line = source[..at.min(source.len())].matches('\n').count();
        format!("{} at line {}: {}", error.message, line + 1, source.lines().nth(line).unwrap_or("").trim().chars().take(90).collect::<String>())
    })
}

/// Test builds: the program as oxc sees it.
#[cfg(feature = "test-api")]
pub fn tree_of(path: &str, source: &str) -> String {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type(path)).parse();
    format!("{:#?}", parsed.program.body)
}
