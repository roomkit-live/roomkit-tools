use std::ops::ControlFlow;
use std::time::Instant;

use roomkit_tools_core::ToolError;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use tree_sitter::{Node, ParseOptions, Parser};

use crate::{CodeLanguage, CodeOutlineInput, CodeOutlineLimits, CodeSymbol, SymbolKind};

pub(crate) struct Control {
    pub(crate) token: CancellationToken,
    pub(crate) deadline: Instant,
}

impl Control {
    pub(crate) fn check(&self) -> Result<(), ToolError> {
        if self.token.is_cancelled() {
            Err(ToolError::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(ToolError::LimitExceeded(
                "code outline deadline exceeded".into(),
            ))
        } else {
            Ok(())
        }
    }
}

pub(crate) struct SourceOutline {
    pub(crate) symbols: Vec<CodeSymbol>,
    pub(crate) revision: String,
    pub(crate) syntax_errors: bool,
}

pub(crate) fn validate_revision(expected: Option<&str>) -> Result<(), ToolError> {
    if let Some(value) = expected {
        let digest = value.strip_prefix("sha256:").ok_or_else(invalid_revision)?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(invalid_revision());
        }
    }
    Ok(())
}

fn invalid_revision() -> ToolError {
    ToolError::InvalidInput(
        "expected_revision must be sha256: followed by 64 lowercase hexadecimal digits".into(),
    )
}

pub(crate) fn outline(
    text: &str,
    language: CodeLanguage,
    input: &CodeOutlineInput,
    limits: CodeOutlineLimits,
    control: &Control,
) -> Result<SourceOutline, ToolError> {
    control.check()?;
    let revision = roomkit_tools_core::revision::sha256(&Sha256::digest(text.as_bytes()));
    if input
        .expected_revision
        .as_ref()
        .is_some_and(|expected| expected != &revision)
    {
        return Err(ToolError::AccessRefused(
            "source changed since expected_revision; read or outline it again".into(),
        ));
    }
    // A UTF-8 BOM is not source syntax; it still participates in the revision.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut parser = Parser::new();
    let grammar = match language {
        CodeLanguage::Rust => tree_sitter_rust::LANGUAGE.into(),
        CodeLanguage::Python => tree_sitter_python::LANGUAGE.into(),
    };
    parser
        .set_language(&grammar)
        .map_err(|_| ToolError::UnsupportedResource("incompatible source grammar".into()))?;
    let mut progress = |_: &tree_sitter::ParseState| {
        if control.check().is_err() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = ParseOptions::new().progress_callback(&mut progress);
    let tree = parser.parse_with_options(
        &mut |offset, _| text.as_bytes().get(offset..).unwrap_or_default(),
        None,
        Some(options),
    );
    control.check()?;
    let tree = tree.ok_or_else(|| {
        ToolError::UnsupportedResource("source parser did not produce a tree".into())
    })?;
    let symbols = walk(tree.root_node(), text, limits, control)?;
    Ok(SourceOutline {
        symbols,
        revision,
        syntax_errors: tree.root_node().has_error(),
    })
}

fn walk(
    root: Node<'_>,
    text: &str,
    limits: CodeOutlineLimits,
    control: &Control,
) -> Result<Vec<CodeSymbol>, ToolError> {
    let mut cursor = root.walk();
    let mut depth = 0;
    let mut visited = 0;
    let mut scopes: Vec<(usize, usize, SymbolKind)> = Vec::new();
    let mut symbols = Vec::new();
    'walk: loop {
        control.check()?;
        visited += 1;
        if visited > limits.max_scan_nodes || depth > limits.max_depth {
            return Err(ToolError::LimitExceeded(
                "syntax traversal exceeds max_scan_nodes or max_depth".into(),
            ));
        }
        while scopes.last().is_some_and(|scope| scope.0 >= depth) {
            scopes.pop();
        }
        let parent = scopes.last().map(|scope| (scope.1, scope.2));
        if let Some(symbol) = definition(cursor.node(), text, parent, symbols.len() + 1, limits)? {
            if symbols.len() >= limits.max_scan_symbols {
                return Err(ToolError::LimitExceeded(
                    "source exceeds max_scan_symbols".into(),
                ));
            }
            scopes.push((depth, symbol.index, symbol.kind));
            symbols.push(symbol);
        }
        if cursor.goto_first_child() {
            depth += 1;
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                break 'walk;
            }
            depth -= 1;
        }
    }
    Ok(symbols)
}

fn definition(
    node: Node<'_>,
    text: &str,
    parent: Option<(usize, SymbolKind)>,
    index: usize,
    limits: CodeOutlineLimits,
) -> Result<Option<CodeSymbol>, ToolError> {
    let kind = match node.kind() {
        "function_item" | "function_signature_item" | "function_definition" => {
            if parent.is_some_and(|(_, kind)| {
                matches!(
                    kind,
                    SymbolKind::Class | SymbolKind::Impl | SymbolKind::Trait
                )
            }) {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            }
        }
        "class_definition" => SymbolKind::Class,
        "struct_item" => SymbolKind::Struct,
        "enum_item" => SymbolKind::Enum,
        "trait_item" => SymbolKind::Trait,
        "impl_item" => SymbolKind::Impl,
        "mod_item" => SymbolKind::Module,
        "type_item" => SymbolKind::TypeAlias,
        _ => return Ok(None),
    };
    let name_node = node.child_by_field_name(if kind == SymbolKind::Impl {
        "type"
    } else {
        "name"
    });
    let Some(name_node) = name_node.filter(|n| !n.is_missing()) else {
        return Ok(None);
    };
    let name = slice(text, name_node.start_byte(), name_node.end_byte())?;
    if name.len() > limits.max_signature_bytes {
        return Err(ToolError::LimitExceeded(
            "definition name exceeds max_signature_bytes".into(),
        ));
    }
    let start = definition_start(node);
    let header_end = node
        .child_by_field_name("body")
        .map_or(node.end_byte(), |body| body.start_byte());
    let header = slice(text, start.start_byte(), header_end)?.trim_end();
    let mut end = header.len().min(limits.max_signature_bytes);
    while !header.is_char_boundary(end) {
        end -= 1;
    }
    let position = node.end_position();
    let end_line = position.row + usize::from(position.column != 0 || position.row == 0);
    Ok(Some(CodeSymbol {
        index,
        parent: parent.map(|(index, _)| index),
        kind,
        name: name.into(),
        signature: slice(header, 0, end)?.into(),
        signature_truncated: end < header.len(),
        start_line: start.start_position().row + 1,
        end_line,
    }))
}

fn definition_start(mut node: Node<'_>) -> Node<'_> {
    if let Some(parent) = node
        .parent()
        .filter(|parent| parent.kind() == "decorated_definition")
    {
        return parent;
    }
    while let Some(previous) = node
        .prev_named_sibling()
        .filter(|n| n.kind() == "attribute_item")
    {
        node = previous;
    }
    node
}

fn slice(text: &str, start: usize, end: usize) -> Result<&str, ToolError> {
    text.get(start..end).ok_or_else(|| {
        ToolError::UnsupportedResource("invalid source byte range from parser".into())
    })
}
