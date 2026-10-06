// PHP declaration boundaries for semantic search. Malformed trees fall back to
// text; class bodies are not giant selection units that swallow every method.
use super::source::{Declared, Range};
use tree_sitter::{Node, Parser};

pub struct Parsed {
    pub units: Vec<Declared>,
    pub comments: Vec<Range>,
}

fn range(node: Node) -> Range {
    let end = node.end_position();
    let start = node.start_position().row + 1;
    Range::new(start, if end.column == 0 { end.row.max(start) } else { end.row + 1 })
}

fn visit(node: Node, source: &str, owner: Option<&str>, headers: &[Range], parsed: &mut Parsed) {
    let kind = node.kind();
    if kind == "comment" {
        parsed.comments.push(range(node));
        return;
    }
    let name = node.child_by_field_name("name").and_then(|n| n.utf8_text(source.as_bytes()).ok());
    if matches!(kind, "class_declaration" | "interface_declaration" | "trait_declaration" | "enum_declaration") {
        if let (Some(name), Some(body)) = (name, node.child_by_field_name("body")) {
            let header = Range::new(node.start_position().row + 1, body.start_position().row + 1);
            parsed.units.push(Declared { name: name.into(), range: header, owner_headers: headers.to_vec() });
            let mut context = headers.to_vec();
            context.push(header);
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                visit(child, source, Some(name), &context, parsed);
            }
            return;
        }
    }
    if matches!(kind, "function_definition" | "method_declaration") {
        if let Some(name) = name {
            let qualified = if kind == "method_declaration" {
                owner.map_or_else(|| name.to_string(), |owner| format!("{owner}::{name}"))
            } else { name.to_string() };
            parsed.units.push(Declared { name: qualified, range: range(node), owner_headers: headers.to_vec() });
        }
    } else if matches!(kind, "property_declaration" | "const_declaration" | "enum_case") {
        let label = match kind { "property_declaration" => "properties", "enum_case" => "case", _ => "constants" };
        parsed.units.push(Declared {
            name: owner.map_or_else(|| label.to_string(), |owner| format!("{owner}::{label}")),
            range: range(node), owner_headers: headers.to_vec(),
        });
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, source, owner, headers, parsed);
    }
}

pub fn declarations(source: &str) -> Option<Parsed> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_php::LANGUAGE_PHP.into()).ok()?;
    let tree = parser.parse(source, None)?;
    if tree.root_node().has_error() { return None; }
    let mut parsed = Parsed { units: Vec::new(), comments: Vec::new() };
    visit(tree.root_node(), source, None, &[], &mut parsed);
    parsed.units.sort_by_key(|unit| (unit.range.start, unit.range.end));
    Some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_are_separate_units_with_class_context() {
        let source = "<?php\nnamespace App;\n/** Finds evidence. */\nfinal class QuoteLocator\n{\n    public function unrelated() { return null; }\n\n    public function locate(string $quote): int\n    {\n        return strpos('example', $quote);\n    }\n}\n";
        let parsed = declarations(source).unwrap();
        let locate = parsed.units.iter().find(|u| u.name == "QuoteLocator::locate").unwrap();
        assert_eq!(locate.range, Range::new(8, 11));
        assert_eq!(locate.owner_headers, vec![Range::new(4, 5)]);
        assert_eq!(parsed.units[0].range, Range::new(4, 5));
        assert_eq!(parsed.comments, vec![Range::new(3, 3)]);
        assert!(!parsed.units.iter().any(|u| u.range == Range::new(4, 12)));
    }

    #[test]
    fn attributes_interfaces_traits_and_functions_are_inspected() {
        let source = "<?php\n#[Example]\nclass Panel { public const TITLE = 'Quote'; private string $text; }\ninterface Locator { public function locate(string $text): int; }\ntrait Shared { public function shared() {} }\nfunction locate_quote($text) { return $text; }\n";
        let parsed = declarations(source).unwrap();
        for name in ["Panel", "Panel::constants", "Panel::properties", "Locator::locate", "Shared::shared", "locate_quote"] {
            assert!(parsed.units.iter().any(|u| u.name == name), "missing {name}");
        }
    }

    #[test]
    fn inspection_locates_a_method_after_a_large_unrelated_body() {
        use super::super::source::{inspect, Bounds, Fallback, Mode};
        let source = format!("<?php\nclass Controller {{\n    function unrelated() {{\n{}    }}\n    function locateQuote() {{ return 'quote evidence'; }}\n}}\n", "        $unused = 0;\n".repeat(250));
        let inspected = inspect("Controller.php", &source, Bounds::default());
        assert_eq!(inspected.mode, Mode::Php);
        assert!(inspected.fallback.is_none());
        let unit = inspected.units.iter().find(|u| u.name == "Controller::locateQuote").unwrap();
        assert!(unit.range.start > 250);
        assert!(!unit.partial);
        let body = &source[unit.byte_start..unit.byte_end];
        assert!(body.contains("quote evidence"));
        assert!(!body.contains("$unused"));
        let broken = inspect("Broken.php", "<?php class {", Bounds::default());
        assert_eq!(broken.fallback, Some(Fallback::Syntax));
    }

    #[test]
    fn syntax_errors_do_not_produce_trusted_method_ranges() {
        assert!(declarations("<?php class Broken { public function locate( {").is_none());
    }
}
