//! Interception: raw Python source in, structured observations out. All parsing
//! is done with tree-sitter, so partially broken files still yield whatever the
//! parser could recover.

use anyhow::Result;
use tree_sitter::{Node, Parser};

use crate::dossier::{Operation, Subject};

/// Everything lifted out of one source file.
#[derive(Default)]
pub struct Sweep {
    pub subjects: Vec<Subject>,
    pub operations: Vec<Operation>,
}

pub struct Interceptor {
    parser: Parser,
}

impl Interceptor {
    pub fn new() -> Result<Self> {
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_python::LANGUAGE.into())?;
        Ok(Self { parser })
    }

    /// Extract every class and function definition in a single source file.
    pub fn sweep(&mut self, source: &str, module: &str, file: &str) -> Sweep {
        let Some(tree) = self.parser.parse(source, None) else {
            return Sweep::default();
        };
        let mut sweep = Sweep::default();
        let scope = Scope {
            module,
            file,
            prefix: module,
            owner: None,
        };
        walk(tree.root_node(), source.as_bytes(), &scope, &[], &mut sweep);
        sweep
    }
}

/// Where in the file we currently are: what names definitions hang off, and
/// which class (if any) owns them.
struct Scope<'a> {
    module: &'a str,
    file: &'a str,
    prefix: &'a str,
    owner: Option<&'a str>,
}

fn walk(node: Node, src: &[u8], scope: &Scope, decorators: &[String], out: &mut Sweep) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            // Carry the decorators down to the definition they apply to.
            "decorated_definition" => {
                let marks = decorators_of(child, src);
                if let Some(def) = child.child_by_field_name("definition") {
                    walk_one(def, src, scope, &marks, out);
                }
            }
            _ => walk_one(child, src, scope, decorators, out),
        }
    }
}

fn walk_one(node: Node, src: &[u8], scope: &Scope, decorators: &[String], out: &mut Sweep) {
    match node.kind() {
        "class_definition" => {
            let subject = capture_class(node, src, scope);
            let qualname = subject.qualname.clone();
            out.subjects.push(subject);
            if let Some(body) = node.child_by_field_name("body") {
                let inner = Scope {
                    module: scope.module,
                    file: scope.file,
                    prefix: &qualname,
                    owner: Some(&qualname),
                };
                walk(body, src, &inner, &[], out);
            }
        }
        "function_definition" => {
            let operation = capture_function(node, src, scope, decorators);
            let callsign = operation.callsign.clone();
            out.operations.push(operation);
            // A nested definition belongs to the enclosing function, not to the
            // class the outer function is a method of.
            if let Some(body) = node.child_by_field_name("body") {
                let inner = Scope {
                    module: scope.module,
                    file: scope.file,
                    prefix: &callsign,
                    owner: None,
                };
                walk(body, src, &inner, &[], out);
            }
        }
        _ => walk(node, src, scope, decorators, out),
    }
}

fn capture_class(node: Node, src: &[u8], scope: &Scope) -> Subject {
    let name = node
        .child_by_field_name("name")
        .map(|n| text(n, src))
        .unwrap_or_else(|| "<unnamed>".to_string());

    Subject {
        qualname: format!("{}.{name}", scope.prefix),
        name,
        module: scope.module.to_string(),
        file: scope.file.to_string(),
        line: node.start_position().row + 1,
        bases: node
            .child_by_field_name("superclasses")
            .map(|n| bases_of(n, src))
            .unwrap_or_default(),
        methods: methods_of(node, src),
    }
}

fn capture_function(node: Node, src: &[u8], scope: &Scope, decorators: &[String]) -> Operation {
    let name = node
        .child_by_field_name("name")
        .map(|n| text(n, src))
        .unwrap_or_else(|| "<unnamed>".to_string());

    Operation {
        callsign: format!("{}.{name}", scope.prefix),
        name,
        module: scope.module.to_string(),
        owner: scope.owner.map(|o| o.to_string()),
        file: scope.file.to_string(),
        line: node.start_position().row + 1,
        params: params_of(node, src),
        decorators: decorators.to_vec(),
        is_async: node.child(0).is_some_and(|c| c.kind() == "async"),
        raw_calls: calls_of(node, src),
        links: Vec::new(),
        external: 0,
        recursive: false,
    }
}

/// Pull base classes out of the argument list, dropping the parts that do not
/// describe inheritance: `metaclass=`, `**kwargs`, generic subscripts.
fn bases_of(list: Node, src: &[u8]) -> Vec<String> {
    let mut cursor = list.walk();
    let mut bases = Vec::new();
    for arg in list.named_children(&mut cursor) {
        let base = match arg.kind() {
            "identifier" | "attribute" => Some(text(arg, src)),
            // `Generic[T]` inherits from `Generic`.
            "subscript" => arg.child_by_field_name("value").map(|n| text(n, src)),
            // `Base(...)` shows up with dynamic base construction.
            "call" => arg.child_by_field_name("function").map(|n| text(n, src)),
            _ => None,
        };
        if let Some(base) = base {
            bases.push(base);
        }
    }
    bases
}

fn methods_of(class: Node, src: &[u8]) -> Vec<String> {
    let Some(body) = class.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut cursor = body.walk();
    let mut methods = Vec::new();
    for stmt in body.named_children(&mut cursor) {
        let def = if stmt.kind() == "decorated_definition" {
            stmt.child_by_field_name("definition")
        } else {
            Some(stmt)
        };
        if let Some(def) = def {
            if def.kind() == "function_definition" {
                if let Some(name) = def.child_by_field_name("name") {
                    methods.push(text(name, src));
                }
            }
        }
    }
    methods.sort();
    methods
}

fn decorators_of(node: Node, src: &[u8]) -> Vec<String> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|c| c.kind() == "decorator")
        .map(|c| text(c, src).trim_start_matches('@').to_string())
        .collect()
}

fn params_of(node: Node, src: &[u8]) -> Vec<String> {
    let Some(params) = node.child_by_field_name("parameters") else {
        return Vec::new();
    };
    let mut cursor = params.walk();
    params
        .named_children(&mut cursor)
        .filter(|p| p.kind() != "comment")
        .map(|p| match p.kind() {
            "identifier" => text(p, src),
            "list_splat_pattern" => format!("*{}", child_text(p, src)),
            "dictionary_splat_pattern" => format!("**{}", child_text(p, src)),
            // `typed_parameter`, `default_parameter`, ...: keep the name only,
            // so a retyped or re-defaulted parameter is not reported as a change.
            _ => p
                .child_by_field_name("name")
                .map(|n| text(n, src))
                .unwrap_or_else(|| child_text(p, src)),
        })
        .collect()
}

/// Every call made directly by this function, in source order, deduplicated.
/// Nested definitions are skipped: their calls belong to them.
fn calls_of(function: Node, src: &[u8]) -> Vec<String> {
    let Some(body) = function.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut found = Vec::new();
    descend(body, src, &mut found);
    let mut seen = std::collections::BTreeSet::new();
    found.retain(|c| seen.insert(c.clone()));
    found
}

fn descend(node: Node, src: &[u8], out: &mut Vec<String>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" | "class_definition" | "decorated_definition" => continue,
            "call" => {
                if let Some(target) = child.child_by_field_name("function") {
                    // Only plain names and attribute chains are worth recording;
                    // anything else is not something we could resolve.
                    if matches!(target.kind(), "identifier" | "attribute") {
                        out.push(text(target, src));
                    }
                }
                // Arguments can contain further calls.
                descend(child, src, out);
            }
            _ => descend(child, src, out),
        }
    }
}

fn text(node: Node, src: &[u8]) -> String {
    node.utf8_text(src).unwrap_or_default().to_string()
}

fn child_text(node: Node, src: &[u8]) -> String {
    node.named_child(0)
        .map(|c| text(c, src))
        .unwrap_or_else(|| text(node, src))
}
