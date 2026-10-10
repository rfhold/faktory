//! Bounded, passive companion compilation. No project sources outside this namespace are emitted.

#![allow(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "asset identities and supported extensions are deliberately case-sensitive"
)]

use std::collections::BTreeSet;

use cssparser::{Parser, ParserInput, Token};
use html5ever::{parse_document, tendril::TendrilSink as _};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use serde::Serialize;

use crate::model::project::ProjectFile;

const MAX_INPUT: usize = 512 * 1024;
const MAX_OUTPUT: usize = 1024 * 1024;
const MAX_ASSETS: usize = 64;
const MAX_NODES: usize = 16_384;
const MAX_DEPTH: usize = 64;

#[derive(Debug, Serialize)]
pub struct Companion {
    pub format: &'static str,
    pub revision: String,
    pub kit_version: &'static str,
    pub html: String,
    pub css: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Serialize)]
pub struct Asset {
    pub id: String,
    pub mime_type: &'static str,
    pub content: String,
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("companion contains unsupported or invalid content")]
pub struct CompileError;

pub fn compile(files: &[ProjectFile], revision: &str) -> Result<Option<Companion>, CompileError> {
    let Some(index) = files
        .iter()
        .find(|file| file.path == "companion/index.html")
    else {
        return Ok(None);
    };
    let mut total = 0_usize;
    let mut assets = Vec::new();
    let mut ids = BTreeSet::new();
    let mut css = String::new();
    for file in files
        .iter()
        .filter(|file| file.path.starts_with("companion/"))
    {
        total = total.checked_add(file.content.len()).ok_or(CompileError)?;
        if total > MAX_INPUT {
            return Err(CompileError);
        }
        match file.path.as_str() {
            "companion/index.html" => {}
            "companion/style.css" => {
                validate_css(&file.content, false)?;
                css.push_str(&file.content);
                css.push('\n');
            }
            path => {
                let id = path.strip_prefix("companion/").ok_or(CompileError)?;
                validate_asset_id(id)?;
                if assets.len() >= MAX_ASSETS || !ids.insert(id.to_owned()) {
                    return Err(CompileError);
                }
                let (mime_type, content) = if id.ends_with(".svg") {
                    ("image/svg+xml", compile_svg(&file.content)?)
                } else if id.ends_with(".json") {
                    let value: serde_json::Value =
                        serde_json::from_str(&file.content).map_err(|_| CompileError)?;
                    (
                        "application/json",
                        serde_json::to_string(&value).map_err(|_| CompileError)?,
                    )
                } else {
                    return Err(CompileError);
                };
                assets.push(Asset {
                    id: id.to_owned(),
                    mime_type,
                    content,
                });
            }
        }
    }
    let dom = parse_document(RcDom::default(), html5ever::ParseOpts::default())
        .one(index.content.as_str());
    let mut html = String::new();
    let mut count = 0;
    compile_html(&dom.document, &ids, &mut html, &mut css, 0, &mut count)?;
    let payload = Companion {
        format: "faktory-companion-v1",
        revision: revision.to_owned(),
        kit_version: "1",
        html,
        css,
        assets,
    };
    if serde_json::to_vec(&payload)
        .map_err(|_| CompileError)?
        .len()
        > MAX_OUTPUT
    {
        return Err(CompileError);
    }
    Ok(Some(payload))
}

fn validate_asset_id(id: &str) -> Result<(), CompileError> {
    if !id.starts_with("assets/")
        || id.len() > 256
        || !id.is_ascii()
        || id.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
    {
        return Err(CompileError);
    }
    Ok(())
}

fn escape(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            _ => output.push(character),
        }
    }
}

fn local_fragment(value: &str) -> bool {
    value.starts_with('#')
        && value.len() > 1
        && value[1..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-:.".contains(&byte))
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "explicit passive HTML allowlist and bounded recursive serializer"
)]
fn compile_html(
    node: &Handle,
    ids: &BTreeSet<String>,
    html: &mut String,
    css: &mut String,
    depth: usize,
    count: &mut usize,
) -> Result<(), CompileError> {
    *count += 1;
    if depth > MAX_DEPTH || *count > MAX_NODES || html.len() + css.len() > MAX_OUTPUT {
        return Err(CompileError);
    }
    let mut close = None;
    match &node.data {
        NodeData::Document | NodeData::Doctype { .. } | NodeData::Comment { .. } => {}
        NodeData::Text { contents } => escape(&contents.borrow(), html),
        NodeData::Element { name, attrs, .. } => {
            if name.ns.as_ref() == "http://www.w3.org/2000/svg" && name.local.as_ref() == "svg" {
                let mut source = String::new();
                svg_dom_source(node, &mut source, depth, count)?;
                html.push_str(&compile_svg(&source)?);
                return Ok(());
            }
            if name.ns.as_ref() != "http://www.w3.org/1999/xhtml" {
                return Err(CompileError);
            }
            let tag = name.local.as_ref();
            if matches!(tag, "html" | "head" | "body") {
                if !attrs.borrow().is_empty() {
                    return Err(CompileError);
                }
            } else if tag == "style" {
                if !attrs.borrow().is_empty() {
                    return Err(CompileError);
                }
                let mut text = String::new();
                for child in node.children.borrow().iter() {
                    if let NodeData::Text { contents } = &child.data {
                        text.push_str(&contents.borrow());
                    } else {
                        return Err(CompileError);
                    }
                }
                validate_css(&text, false)?;
                css.push_str(&text);
                css.push('\n');
                return Ok(());
            } else {
                if !matches!(
                    tag,
                    "div"
                        | "span"
                        | "p"
                        | "section"
                        | "article"
                        | "header"
                        | "footer"
                        | "main"
                        | "nav"
                        | "aside"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "ul"
                        | "ol"
                        | "li"
                        | "dl"
                        | "dt"
                        | "dd"
                        | "table"
                        | "thead"
                        | "tbody"
                        | "tfoot"
                        | "tr"
                        | "th"
                        | "td"
                        | "caption"
                        | "colgroup"
                        | "col"
                        | "figure"
                        | "figcaption"
                        | "pre"
                        | "code"
                        | "blockquote"
                        | "q"
                        | "em"
                        | "strong"
                        | "b"
                        | "i"
                        | "u"
                        | "s"
                        | "small"
                        | "sub"
                        | "sup"
                        | "mark"
                        | "time"
                        | "abbr"
                        | "kbd"
                        | "samp"
                        | "var"
                        | "br"
                        | "hr"
                        | "wbr"
                        | "img"
                        | "a"
                        | "details"
                        | "summary"
                        | "button"
                        | "label"
                        | "output"
                        | "fk-tabs"
                        | "fk-callout"
                        | "fk-measurement"
                        | "fk-steps"
                        | "fk-asset"
                        | "fk-cut-layout"
                ) {
                    return Err(CompileError);
                }
                html.push('<');
                html.push_str(tag);
                let mut asset = false;
                for attr in attrs.borrow().iter() {
                    let key = attr.name.local.as_ref();
                    let value = attr.value.as_ref();
                    if !attr.name.ns.is_empty() {
                        return Err(CompileError);
                    }
                    if key == "style" {
                        validate_css(value, false)?;
                    } else if key == "href" && tag == "a" {
                        if !local_fragment(value) {
                            return Err(CompileError);
                        }
                    } else if key == "data-companion-asset" {
                        validate_asset_id(value)?;
                        if !ids.contains(value)
                            || !match tag {
                                "img" => value.ends_with(".svg"),
                                "fk-cut-layout" => value.ends_with(".json"),
                                "fk-asset" => true,
                                _ => false,
                            }
                        {
                            return Err(CompileError);
                        }
                        asset = true;
                    } else if !(matches!(
                        key,
                        "id" | "class" | "title" | "lang" | "dir" | "role" | "hidden" | "tabindex"
                    ) || key.starts_with("aria-")
                        || key.starts_with("data-")
                        || (tag == "img" && matches!(key, "alt" | "width" | "height"))
                        || (matches!(tag, "th" | "td")
                            && matches!(key, "colspan" | "rowspan" | "scope" | "headers"))
                        || (matches!(tag, "ol" | "li")
                            && matches!(key, "start" | "reversed" | "value"))
                        || (tag == "details" && key == "open")
                        || (tag == "time" && key == "datetime")
                        || (tag == "fk-measurement" && matches!(key, "label" | "value" | "unit"))
                        || (tag == "button" && key == "type" && value == "button"))
                    {
                        return Err(CompileError);
                    }
                    html.push(' ');
                    html.push_str(key);
                    html.push_str("=\"");
                    escape(value, html);
                    html.push('"');
                }
                if matches!(tag, "img" | "fk-asset" | "fk-cut-layout") && !asset {
                    return Err(CompileError);
                }
                html.push('>');
                if !matches!(tag, "br" | "hr" | "wbr" | "img" | "col") {
                    close = Some(tag);
                }
            }
        }
        NodeData::ProcessingInstruction { .. } => return Err(CompileError),
    }
    for child in node.children.borrow().iter() {
        compile_html(child, ids, html, css, depth + 1, count)?;
    }
    if let Some(tag) = close {
        html.push_str("</");
        html.push_str(tag);
        html.push('>');
    }
    Ok(())
}

fn validate_css(css: &str, fragments: bool) -> Result<(), CompileError> {
    let mut input = ParserInput::new(css);
    css_tokens(&mut Parser::new(&mut input), fragments, 0)
}

// Convert the HTML parser's foreign-content tree to XML, then apply the same SVG policy as assets.
fn svg_dom_source(
    node: &Handle,
    output: &mut String,
    depth: usize,
    count: &mut usize,
) -> Result<(), CompileError> {
    *count += 1;
    if depth > MAX_DEPTH || *count > MAX_NODES || output.len() > MAX_OUTPUT {
        return Err(CompileError);
    }
    match &node.data {
        NodeData::Text { contents } => escape(&contents.borrow(), output),
        NodeData::Comment { .. } => {}
        NodeData::Element { name, attrs, .. } => {
            if name.ns.as_ref() != "http://www.w3.org/2000/svg" {
                return Err(CompileError);
            }
            output.push('<');
            output.push_str(&name.local);
            for attr in attrs.borrow().iter() {
                if !(attr.name.ns.is_empty()
                    || (attr.name.ns.as_ref() == "http://www.w3.org/1999/xlink"
                        && attr.name.local.as_ref() == "href")
                    || (attr.name.ns.as_ref() == "http://www.w3.org/2000/xmlns/"
                        && attr.name.local.as_ref() == "xmlns"
                        && attr.value.as_ref() == "http://www.w3.org/2000/svg"))
                {
                    return Err(CompileError);
                }
                output.push(' ');
                output.push_str(&attr.name.local);
                output.push_str("=\"");
                escape(&attr.value, output);
                output.push('"');
            }
            output.push('>');
            for child in node.children.borrow().iter() {
                svg_dom_source(child, output, depth + 1, count)?;
            }
            output.push_str("</");
            output.push_str(&name.local);
            output.push('>');
        }
        _ => return Err(CompileError),
    }
    Ok(())
}

#[allow(clippy::too_many_lines, reason = "explicit CSS function allowlist")]
fn css_tokens(
    parser: &mut Parser<'_, '_>,
    fragments: bool,
    depth: usize,
) -> Result<(), CompileError> {
    if depth > MAX_DEPTH {
        return Err(CompileError);
    }
    while !parser.is_exhausted() {
        let token = parser.next().map_err(|_| CompileError)?.clone();
        match token {
            Token::BadUrl(_)
            | Token::BadString(_)
            | Token::CloseParenthesis
            | Token::CloseSquareBracket
            | Token::CloseCurlyBracket => return Err(CompileError),
            Token::UnquotedUrl(value) => {
                if !fragments || !local_fragment(&value) {
                    return Err(CompileError);
                }
            }
            Token::AtKeyword(name) => {
                if !matches!(
                    name.to_ascii_lowercase().as_str(),
                    "media"
                        | "supports"
                        | "container"
                        | "layer"
                        | "keyframes"
                        | "-webkit-keyframes"
                        | "page"
                ) {
                    return Err(CompileError);
                }
            }
            Token::Ident(name) => {
                if matches!(
                    name.to_ascii_lowercase().as_str(),
                    "behavior" | "-moz-binding"
                ) {
                    return Err(CompileError);
                }
            }
            Token::Function(name) => {
                if name.eq_ignore_ascii_case("url") && fragments {
                    parser
                        .parse_nested_block(|nested| {
                            let value = nested.expect_string_cloned()?;
                            if !local_fragment(&value) {
                                return Err(nested.new_custom_error::<(), ()>(()));
                            }
                            nested.expect_exhausted()?;
                            Ok(())
                        })
                        .map_err(|_| CompileError)?;
                } else {
                    if !matches!(
                        name.to_ascii_lowercase().as_str(),
                        "calc"
                            | "min"
                            | "max"
                            | "clamp"
                            | "var"
                            | "rgb"
                            | "rgba"
                            | "hsl"
                            | "hsla"
                            | "hwb"
                            | "lab"
                            | "lch"
                            | "oklab"
                            | "oklch"
                            | "color"
                            | "color-mix"
                            | "linear-gradient"
                            | "radial-gradient"
                            | "conic-gradient"
                            | "repeating-linear-gradient"
                            | "repeating-radial-gradient"
                            | "translate"
                            | "translatex"
                            | "translatey"
                            | "translatez"
                            | "translate3d"
                            | "scale"
                            | "scalex"
                            | "scaley"
                            | "scale3d"
                            | "rotate"
                            | "rotatex"
                            | "rotatey"
                            | "rotatez"
                            | "rotate3d"
                            | "skew"
                            | "skewx"
                            | "skewy"
                            | "matrix"
                            | "matrix3d"
                            | "perspective"
                            | "blur"
                            | "brightness"
                            | "contrast"
                            | "drop-shadow"
                            | "grayscale"
                            | "hue-rotate"
                            | "invert"
                            | "opacity"
                            | "saturate"
                            | "sepia"
                            | "cubic-bezier"
                            | "steps"
                            | "is"
                            | "where"
                            | "not"
                            | "has"
                            | "nth-child"
                            | "nth-of-type"
                            | "repeat"
                            | "minmax"
                            | "fit-content"
                    ) {
                        return Err(CompileError);
                    }
                    parser
                        .parse_nested_block(|nested| {
                            css_tokens(nested, fragments, depth + 1)
                                .map_err(|_| nested.new_custom_error::<(), ()>(()))
                        })
                        .map_err(|_| CompileError)?;
                }
            }
            Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock => {
                parser
                    .parse_nested_block(|nested| {
                        css_tokens(nested, fragments, depth + 1)
                            .map_err(|_| nested.new_custom_error::<(), ()>(()))
                    })
                    .map_err(|_| CompileError)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn compile_svg(source: &str) -> Result<String, CompileError> {
    preflight_svg(source)?;
    let document = roxmltree::Document::parse_with_options(
        source,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: u32::try_from(MAX_NODES + 1).map_err(|_| CompileError)?,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .map_err(|_| CompileError)?;
    let root = document.root_element();
    if root.tag_name().name() != "svg" {
        return Err(CompileError);
    }
    let mut output = String::new();
    let mut count = 0;
    svg_node(root, &mut output, 0, &mut count)?;
    Ok(output)
}

// Bound XML nesting before roxmltree's recursive tokenizer can consume an attacker-controlled stack.
fn preflight_svg(source: &str) -> Result<(), CompileError> {
    if source.len() > MAX_INPUT {
        return Err(CompileError);
    }
    let mut stack = Vec::new();
    let mut nodes = 0_usize;
    for token in xmlparser::Tokenizer::from(source) {
        match token.map_err(|_| CompileError)? {
            xmlparser::Token::ElementStart { prefix, local, .. } => {
                if stack.len() > MAX_DEPTH {
                    return Err(CompileError);
                }
                stack.push((prefix.as_str(), local.as_str()));
                nodes += 1;
            }
            xmlparser::Token::ElementEnd {
                end: xmlparser::ElementEnd::Close(prefix, local),
                ..
            } => {
                if stack.pop() != Some((prefix.as_str(), local.as_str())) {
                    return Err(CompileError);
                }
            }
            xmlparser::Token::ElementEnd {
                end: xmlparser::ElementEnd::Empty,
                ..
            } => {
                stack.pop().ok_or(CompileError)?;
            }
            xmlparser::Token::Text { .. }
            | xmlparser::Token::Cdata { .. }
            | xmlparser::Token::Comment { .. } => {
                if stack.len() > MAX_DEPTH {
                    return Err(CompileError);
                }
                nodes += 1;
            }
            xmlparser::Token::DtdStart { .. }
            | xmlparser::Token::EmptyDtd { .. }
            | xmlparser::Token::EntityDeclaration { .. }
            | xmlparser::Token::DtdEnd { .. }
            | xmlparser::Token::ProcessingInstruction { .. } => return Err(CompileError),
            xmlparser::Token::Declaration { .. }
            | xmlparser::Token::Attribute { .. }
            | xmlparser::Token::ElementEnd {
                end: xmlparser::ElementEnd::Open,
                ..
            } => {}
        }
        if nodes > MAX_NODES {
            return Err(CompileError);
        }
    }
    if stack.is_empty() && nodes > 0 {
        Ok(())
    } else {
        Err(CompileError)
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "explicit passive SVG element and attribute allowlists"
)]
fn svg_node(
    node: roxmltree::Node<'_, '_>,
    output: &mut String,
    depth: usize,
    count: &mut usize,
) -> Result<(), CompileError> {
    *count += 1;
    if depth > MAX_DEPTH || *count > MAX_NODES || output.len() > MAX_OUTPUT {
        return Err(CompileError);
    }
    if node.is_text() {
        escape(node.text().unwrap_or_default(), output);
        return Ok(());
    }
    if node.is_comment() {
        return Ok(());
    }
    if !node.is_element() {
        return Err(CompileError);
    }
    let name = node.tag_name();
    if name
        .namespace()
        .is_some_and(|ns| ns != "http://www.w3.org/2000/svg")
        || !matches!(
            name.name(),
            "svg"
                | "g"
                | "path"
                | "rect"
                | "circle"
                | "ellipse"
                | "line"
                | "polyline"
                | "polygon"
                | "text"
                | "tspan"
                | "defs"
                | "marker"
                | "linearGradient"
                | "radialGradient"
                | "stop"
                | "clipPath"
                | "mask"
                | "pattern"
                | "use"
                | "symbol"
                | "title"
                | "desc"
                | "style"
        )
    {
        return Err(CompileError);
    }
    if name.name() == "style" {
        let mut css = String::new();
        for child in node.children() {
            if !child.is_text() {
                return Err(CompileError);
            }
            css.push_str(child.text().unwrap_or_default());
        }
        validate_css(&css, true)?;
    }
    output.push('<');
    output.push_str(name.name());
    if depth == 0 {
        output.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
    }
    for attr in node.attributes() {
        let key = attr.name();
        if attr
            .namespace()
            .is_some_and(|ns| ns != "http://www.w3.org/1999/xlink")
        {
            return Err(CompileError);
        }
        if key == "href" {
            if !local_fragment(attr.value()) {
                return Err(CompileError);
            }
        } else if matches!(
            key,
            "style"
                | "fill"
                | "stroke"
                | "clip-path"
                | "mask"
                | "marker-start"
                | "marker-mid"
                | "marker-end"
        ) {
            validate_css(attr.value(), true)?;
        } else if !matches!(
            key,
            "id" | "class"
                | "viewBox"
                | "preserveAspectRatio"
                | "x"
                | "y"
                | "x1"
                | "y1"
                | "x2"
                | "y2"
                | "cx"
                | "cy"
                | "r"
                | "rx"
                | "ry"
                | "width"
                | "height"
                | "d"
                | "points"
                | "transform"
                | "opacity"
                | "fill-opacity"
                | "fill-rule"
                | "stroke-opacity"
                | "stroke-width"
                | "stroke-linecap"
                | "stroke-linejoin"
                | "stroke-miterlimit"
                | "stroke-dasharray"
                | "stroke-dashoffset"
                | "font-family"
                | "font-size"
                | "font-weight"
                | "font-style"
                | "text-anchor"
                | "dominant-baseline"
                | "dx"
                | "dy"
                | "rotate"
                | "textLength"
                | "lengthAdjust"
                | "gradientUnits"
                | "gradientTransform"
                | "spreadMethod"
                | "offset"
                | "stop-color"
                | "stop-opacity"
                | "fx"
                | "fy"
                | "fr"
                | "clipPathUnits"
                | "maskUnits"
                | "maskContentUnits"
                | "patternUnits"
                | "patternContentUnits"
                | "patternTransform"
                | "markerWidth"
                | "markerHeight"
                | "refX"
                | "refY"
                | "orient"
                | "markerUnits"
                | "vector-effect"
        ) {
            return Err(CompileError);
        }
        output.push(' ');
        output.push_str(key);
        output.push_str("=\"");
        escape(attr.value(), output);
        output.push('"');
    }
    output.push('>');
    for child in node.children() {
        svg_node(child, output, depth + 1, count)?;
    }
    output.push_str("</");
    output.push_str(name.name());
    output.push('>');
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, content: &str) -> ProjectFile {
        ProjectFile {
            path: path.to_owned(),
            content: content.to_owned(),
        }
    }

    fn page(html: &str) -> Result<Option<Companion>, CompileError> {
        compile(&[file("companion/index.html", html)], "revision")
    }

    #[test]
    fn compiles_actual_kit_fixtures_and_serializes_exact_contract() {
        let output_directory =
            std::env::var_os("COMPANION_TEST_OUTPUT_DIR").map(std::path::PathBuf::from);
        if let Some(directory) = &output_directory {
            assert!(
                directory.is_dir(),
                "COMPANION_TEST_OUTPUT_DIR must be an existing directory"
            );
        }
        for (name, source) in [
            (
                "bench",
                include_str!("../../../packages/companion-kit/fixtures/bench.json"),
            ),
            (
                "generic",
                include_str!("../../../packages/companion-kit/fixtures/generic.json"),
            ),
        ] {
            let fixture: serde_json::Value = serde_json::from_str(source).unwrap();
            let mut files = vec![
                file("companion/index.html", fixture["html"].as_str().unwrap()),
                file("companion/style.css", fixture["css"].as_str().unwrap()),
            ];
            for asset in fixture["assets"].as_array().unwrap() {
                files.push(file(
                    &format!("companion/{}", asset["id"].as_str().unwrap()),
                    asset["content"].as_str().unwrap(),
                ));
            }
            let compiled = compile(&files, "fixture-1").unwrap().unwrap();
            let output = serde_json::to_value(&compiled).unwrap();
            assert_eq!(output.as_object().unwrap().len(), 6);
            for key in ["format", "revision", "kit_version"] {
                assert_eq!(output[key], fixture[key]);
            }
            assert_eq!(compiled.css.trim(), fixture["css"].as_str().unwrap());
            assert!(compiled.html.contains("<fk-callout>"));
            assert_eq!(
                compiled.assets.len(),
                fixture["assets"].as_array().unwrap().len()
            );
            for (actual, expected) in compiled
                .assets
                .iter()
                .zip(fixture["assets"].as_array().unwrap())
            {
                assert_eq!(actual.id, expected["id"].as_str().unwrap());
                assert_eq!(actual.mime_type, expected["mime_type"].as_str().unwrap());
                if actual.mime_type == "application/json" {
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(&actual.content).unwrap(),
                        serde_json::from_str::<serde_json::Value>(
                            expected["content"].as_str().unwrap()
                        )
                        .unwrap()
                    );
                } else {
                    assert!(actual.content.starts_with("<svg xmlns="));
                }
                assert!(
                    compiled
                        .html
                        .contains(&format!("data-companion-asset=\"{}\"", actual.id))
                );
            }
            if let Some(directory) = &output_directory {
                let bytes = serde_json::to_vec(&compiled).unwrap();
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                    output
                );
                std::fs::write(directory.join(format!("{name}.json")), bytes).unwrap();
            }
        }
    }

    #[test]
    fn embedded_svg_uses_asset_policy_and_preserves_diagrams() {
        let generic: serde_json::Value = serde_json::from_str(include_str!(
            "../../../packages/companion-kit/fixtures/generic.json"
        ))
        .unwrap();
        let html = format!(
            "{}<svg viewBox='0 0 20 20'><defs><linearGradient id='paint'><stop offset='0' stop-color='red'/></linearGradient></defs><rect id='shape' width='20' height='20' fill='url(#paint)'/><use href='#shape'/><text x='1' y='5'>A &amp; B</text></svg>",
            generic["html"].as_str().unwrap()
        );
        let payload = compile(
            &[
                file("companion/index.html", &html),
                file("companion/assets/criteria.json", "{}"),
            ],
            "fixture-1",
        )
        .unwrap()
        .unwrap();
        assert!(
            payload
                .html
                .contains("<svg xmlns=\"http://www.w3.org/2000/svg\"")
        );
        assert!(payload.html.contains("viewBox=\"0 0 20 20\""));
        assert!(payload.html.contains("fill=\"url(#paint)\""));
        assert!(payload.html.contains("A &amp; B"));
        for body in [
            "<script>alert(1)</script>",
            "<foreignObject><p>x</p></foreignObject>",
            "<image href='/private'/>",
            "<use href='https://evil.test/#x'/>",
            "<rect onload='evil()'/>",
            "<style>@import 'https://evil.test/x';</style>",
            "<rect fill='url(https://evil.test/x)'/>",
        ] {
            assert!(
                page(&format!("<svg>{body}</svg>")).is_err(),
                "accepted {body}"
            );
        }
    }

    #[test]
    fn curated_widgets_reject_unknown_elements_and_bad_asset_kinds() {
        assert!(
            page("<fk-measurement label='Length' value='12' unit='mm'></fk-measurement>").is_ok()
        );
        for html in [
            "<fk-unknown></fk-unknown>",
            "<fk-tabs onclick='evil()'></fk-tabs>",
            "<fk-asset></fk-asset>",
            "<fk-cut-layout data-companion-asset='assets/diagram.svg'></fk-cut-layout>",
            "<img data-companion-asset='assets/data.json'>",
            "<fk-asset data-companion-asset='../main.py'></fk-asset>",
        ] {
            assert!(
                compile(
                    &[
                        file("companion/index.html", html),
                        file("companion/assets/diagram.svg", "<svg/>"),
                        file("companion/assets/data.json", "{}")
                    ],
                    "revision"
                )
                .is_err(),
                "accepted {html}"
            );
        }
    }

    #[test]
    fn compiles_layout_styles_diagrams_and_named_data_only() {
        let files = [
            file("main.py", "private-python"),
            file("AGENTS.md", "private-guidance"),
            file("faktory_model/__init__.py", "private-dependency"),
            file(
                "companion/index.html",
                "<style>.grid { display: grid; grid-template-columns: repeat(2, 1fr) }</style><main class='grid'><h1 style='color:rgb(10,20,30)'>Diagram &amp; data</h1><img data-companion-asset='assets/diagram.svg' alt='Diagram'><a href='#details'>Details</a><details id='details'><summary>More</summary><p>Dimensions</p></details></main>",
            ),
            file(
                "companion/style.css",
                "@media (max-width: 600px) { .grid { display: block } }",
            ),
            file(
                "companion/assets/diagram.svg",
                "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 20 20'><defs><linearGradient id='paint'><stop offset='0' stop-color='red'/></linearGradient></defs><rect width='20' height='20' fill='url(#paint)'/><text x='1' y='5'>A &amp; B</text></svg>",
            ),
            file("companion/assets/values.json", "{ \"size\": [1,2,3] }"),
        ];
        let payload = compile(&files, "a".repeat(64).as_str()).unwrap().unwrap();
        assert_eq!(payload.format, "faktory-companion-v1");
        assert_eq!(payload.kit_version, "1");
        assert!(
            payload
                .html
                .contains("data-companion-asset=\"assets/diagram.svg\"")
        );
        assert!(!payload.html.contains("<style"));
        assert!(payload.css.contains("@media"));
        assert!(payload.css.contains("display: grid"));
        assert_eq!(payload.assets.len(), 2);
        assert!(payload.assets[0].content.contains("url(#paint)"));
        let serialized = serde_json::to_string(&payload).unwrap();
        for secret in [
            "private-python",
            "private-guidance",
            "private-dependency",
            "main.py",
            "AGENTS.md",
        ] {
            assert!(!serialized.contains(secret));
        }
    }

    #[test]
    fn rejects_active_html_urls_namespaces_and_asset_traversal() {
        for html in [
            "<script>alert(1)</script>",
            "<div onclick='alert(1)'>x</div>",
            "<img src='https://evil.test/x'>",
            "<img data-companion-asset='../main.py'>",
            "<img data-companion-asset='assets/%2e%2e/x.svg'>",
            "<img data-companion-asset='assets/missing.svg'>",
            "<a href='javascript:alert(1)'>x</a>",
            "<a href='https://evil.test'>x</a>",
            "<a href='/auth/logout'>x</a>",
            "<a href='&#106;avascript:alert(1)'>x</a>",
            "<form action='/oauth/authorize'><button>Go</button></form>",
            "<iframe srcdoc='<script>alert(1)</script>'></iframe>",
            "<object data='/private'></object>",
            "<embed src='/private'>",
            "<meta http-equiv='refresh' content='0;url=https://evil.test'>",
            "<base href='https://evil.test'>",
            "<link rel='stylesheet' href='https://evil.test/x'>",
            "<video poster='https://evil.test'>x</video>",
            "<svg><foreignObject><p>x</p></foreignObject></svg>",
            "<math><mtext>x</mtext></math>",
            "<div background='https://evil.test'>x</div>",
            "<div style='background:u\\72l(https://evil.test)'>x</div>",
        ] {
            assert!(page(html).is_err(), "accepted {html}");
        }
    }

    #[test]
    fn css_parser_rejects_escaped_network_and_execution_constructs() {
        for css in [
            "@page { size: A4; margin: 12mm } @media print { article { break-inside: avoid } }",
            "@-webkit-keyframes reveal { from { opacity: 0 } to { opacity: 1 } }",
            "@supports (display:grid) { @media print { @page { margin: 10mm } } }",
        ] {
            assert!(validate_css(css, false).is_ok(), "rejected {css}");
        }
        for css in [
            "@page { background: url('/private') }",
            "@media print { @page { @import 'https://evil.test/x'; } }",
            "@-webkit-keyframes reveal { from { background: u\\72l('/private') } }",
            "@supports (display:grid) { @media print { div { --secret: url('/private') } } }",
            "@import 'https://evil.test/x';",
            "@\\69mport 'https://evil.test/x';",
            "@font-face { src: url('/private') }",
            "div { background:url(https://evil.test/x) }",
            "div { background:u\\72l('/private') }",
            "div { background:image-set('https://evil.test/x' 1x) }",
            "div { background:-webkit-image-set('https://evil.test/x' 1x) }",
            "div { width:expression(alert(1)) }",
            "div { behavior: 'x' }",
            "div { -moz-binding: 'x' }",
            "div { --secret: url('/private'); color:var(--secret) }",
            "div { background:url(data:image/svg+xml,x) }",
            "div { color:rgb(1,2,url('/private')) }",
            "div { color:'unterminated\n }",
        ] {
            assert!(validate_css(css, false).is_err(), "accepted {css}");
        }
    }

    #[test]
    fn svg_parser_rejects_active_and_external_content() {
        assert!(compile_svg("<svg><style>rect { fill: url('#paint'); stroke: blue }</style><rect style='fill:url(#paint)'/></svg>").is_ok());
        for body in [
            "<script>alert(1)</script>",
            "<foreignObject><div>x</div></foreignObject>",
            "<image href='https://evil.test/x'/>",
            "<use href='https://evil.test/x#id'/>",
            "<path onload='alert(1)'/>",
            "<style>@import 'https://evil.test/x'</style>",
            "<path fill='url(https://evil.test/x)'/>",
            "<path style='fill:u\\72l(https://evil.test/x)'/>",
            "<animate attributeName='href' values='https://evil.test/x'/>",
            "<a href='https://evil.test'>x</a>",
            "<?unsafe instruction?>",
        ] {
            assert!(
                compile_svg(&format!("<svg>{body}</svg>")).is_err(),
                "accepted {body}"
            );
        }
        assert!(
            compile_svg("<!DOCTYPE svg [<!ENTITY x SYSTEM 'file:///private'>]><svg>&x;</svg>")
                .is_err()
        );
        assert!(compile_svg("<svg><g></svg>").is_err());
    }

    #[test]
    fn svg_preflight_depth_and_node_boundaries_are_exact() {
        let nested =
            |depth: usize| format!("<svg>{}{}</svg>", "<g>".repeat(depth), "</g>".repeat(depth));
        assert!(compile_svg(&nested(MAX_DEPTH)).is_ok());
        assert!(preflight_svg(&nested(MAX_DEPTH + 1)).is_err());
        assert!(compile_svg(&nested(MAX_DEPTH + 1)).is_err());
        let siblings = |count: usize| format!("<svg>{}</svg>", "<g/>".repeat(count));
        assert!(compile_svg(&siblings(MAX_NODES - 1)).is_ok());
        assert!(preflight_svg(&siblings(MAX_NODES)).is_err());
        assert!(compile_svg(&siblings(MAX_NODES)).is_err());
        assert!(compile_svg("<?xml version='1.0'?><svg><!--safe--><text>&amp;&lt;&#65;<![CDATA[B]]></text></svg>").is_ok());
        for source in [
            "<!DOCTYPE svg><svg/>",
            "<!DOCTYPE svg [<!ENTITY x 'x'>]><svg>&x;</svg>",
            "<svg><text>&unknown;</text></svg>",
            "<svg><g></svg>",
            "<svg><?unsafe instruction?></svg>",
        ] {
            assert!(compile_svg(source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn deeply_nested_svg_is_rejected_in_isolated_process() {
        const CHILD_ENV: &str = "FAKTORY_COMPANION_DEEP_XML_TEST_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            let svg = format!(
                "<svg>{}{}</svg>",
                "<g>".repeat(70_000),
                "</g>".repeat(70_000)
            );
            let files = [
                file(
                    "companion/index.html",
                    "<img data-companion-asset='assets/deep.svg'>",
                ),
                file("companion/assets/deep.svg", &svg),
            ];
            assert!(files.iter().map(|file| file.content.len()).sum::<usize>() < MAX_INPUT);
            assert!(compile(&files, "revision").is_err());
            assert!(page(&svg).is_err());
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "companion::tests::deeply_nested_svg_is_rejected_in_isolated_process",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated SVG regression failed: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn bounded_input_output_depth_assets_and_namespace() {
        assert!(
            compile(
                &[file("main.py", "x"), file("companion/style.css", "bad")],
                "revision"
            )
            .unwrap()
            .is_none()
        );
        assert!(page(&"x".repeat(MAX_INPUT + 1)).is_err());
        assert!(
            page(&format!(
                "{}x{}",
                "<div>".repeat(MAX_DEPTH + 1),
                "</div>".repeat(MAX_DEPTH + 1)
            ))
            .is_err()
        );
        assert!(page(&"&".repeat(MAX_INPUT)).is_err());
        for path in [
            "companion/assets/../secret.svg",
            "companion/assets/a.js",
            "companion/main.py",
            "companion/assets/x%2f.svg",
        ] {
            assert!(
                compile(
                    &[file("companion/index.html", "<p>x</p>"), file(path, "x")],
                    "revision"
                )
                .is_err()
            );
        }
        assert!(
            compile(
                &[
                    file("companion/index.html", "<p>x</p>"),
                    file("companion/assets/data.json", "{broken")
                ],
                "revision"
            )
            .is_err()
        );
        let mut files = vec![file("companion/index.html", "<p>x</p>")];
        files.extend(
            (0..=MAX_ASSETS).map(|index| file(&format!("companion/assets/{index}.json"), "{}")),
        );
        assert!(compile(&files, "revision").is_err());
        assert_eq!(
            CompileError.to_string(),
            "companion contains unsupported or invalid content"
        );
    }
}
