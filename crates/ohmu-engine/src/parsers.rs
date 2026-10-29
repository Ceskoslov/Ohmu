//! Versioned local parsers. No fetching, JavaScript execution or OCR.

use std::fmt;
use std::path::Path;

use dom_smoothie::Readability;
use ohmu_core::{
    ContentHash, DocumentNode, MAX_PDF_PAGES, MAX_TEXT_BYTES, ParsedDocument, SOURCE_FORMAT_VERSION,
};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::EngineError;
use crate::sources::source_error;

pub(crate) const MAX_NODES: usize = 20_000;
const MARKDOWN_PARSER: &str = "ohmu-markdown/1;pulldown-cmark/0.13.5";
const HTML_PARSER: &str = "ohmu-html/1;dom_smoothie/0.18.2;dom_query/0.28.0;pulldown-cmark/0.13.5";
const PDF_PARSER: &str = "ohmu-pdf/1;pdf-extract/0.12.1;lopdf/0.42.0";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ParserKind {
    Markdown,
    Html,
    Pdf,
}

impl ParserKind {
    pub(crate) fn for_path(path: &Path) -> Result<Self, EngineError> {
        match path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("md" | "markdown") => Ok(Self::Markdown),
            Some("html" | "htm") => Ok(Self::Html),
            Some("pdf") => Ok(Self::Pdf),
            _ => Err(source_error(
                "supported local source extensions: .md, .markdown, .html, .htm, .pdf; URL fetching is not implemented",
            )),
        }
    }

    pub(crate) fn for_identity(identity: &str) -> Option<Self> {
        match identity {
            MARKDOWN_PARSER => Some(Self::Markdown),
            HTML_PARSER => Some(Self::Html),
            PDF_PARSER => Some(Self::Pdf),
            _ => None,
        }
    }

    pub(crate) fn identity(self) -> &'static str {
        match self {
            Self::Markdown => MARKDOWN_PARSER,
            Self::Html => HTML_PARSER,
            Self::Pdf => PDF_PARSER,
        }
    }

    pub(crate) fn parse(self, bytes: &[u8]) -> Result<ParsedDocument, EngineError> {
        // Some malformed PDF structures can panic in the third-party extractor.
        // Catch unwinding before any source evidence is committed. This is not
        // a CPU/memory/process sandbox; the host's panic hook still owns logging.
        let mut document = match self {
            Self::Markdown => parse_markdown(bytes),
            Self::Html => parse_html(bytes),
            Self::Pdf => std::panic::catch_unwind(|| parse_pdf(bytes))
                .map_err(|_| source_error("PDF parser rejected a malformed document"))?,
        }?;
        if self != Self::Markdown {
            document.source_revision =
                Some(ContentHash::new(crate::content_hash(bytes)).map_err(source_error)?);
        }
        self.validate_raw(bytes, &document)?;
        Ok(document)
    }

    pub(crate) fn validate_raw(
        self,
        raw: &[u8],
        document: &ParsedDocument,
    ) -> Result<(), EngineError> {
        if document.parser != self.identity() {
            return Err(source_error("parser output identity mismatch"));
        }
        if self != Self::Markdown
            && document.source_revision.as_ref().map(ContentHash::as_str)
                != Some(crate::content_hash(raw).as_str())
        {
            return Err(source_error(
                "normalized artifact does not match its raw source revision",
            ));
        }
        match self {
            Self::Markdown if document.text.as_bytes() != raw => Err(source_error(
                "Markdown artifact disagrees with its raw source",
            )),
            Self::Html => {
                let text = std::str::from_utf8(raw).map_err(source_error)?;
                validate_html_signature(text)
            }
            Self::Pdf => {
                validate_pdf_signature(raw)?;
                if document.nodes.len() < 2 || document.nodes.len() > MAX_PDF_PAGES + 1 {
                    return Err(source_error("invalid PDF page artifact"));
                }
                let root = &document.nodes[0];
                if root.page.is_some() || root.start != 0 || root.end != 0 || root.id != "root" {
                    return Err(source_error("invalid PDF root node"));
                }
                let mut end = 0;
                for (index, node) in document.nodes[1..].iter().enumerate() {
                    if node.page != Some(index as u32 + 1)
                        || node.start != end
                        || node.parent.as_deref() != Some("root")
                    {
                        return Err(source_error("invalid PDF physical page boundaries"));
                    }
                    end = node.end;
                }
                if end != document.text.len() {
                    return Err(source_error("PDF page ranges do not cover the artifact"));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

fn validate_html_signature(text: &str) -> Result<(), EngineError> {
    // Require actual markup rather than accepting arbitrary text solely by suffix.
    let lower = text.to_ascii_lowercase();
    if text.contains('\0')
        || ![
            "<!doctype html",
            "<html",
            "<body",
            "<article",
            "<main",
            "<div",
            "<p>",
            "<p ",
            "<h1",
            "<h2",
            "<h3",
        ]
        .iter()
        .any(|tag| lower.contains(tag))
    {
        return Err(source_error("HTML source has no supported document markup"));
    }
    Ok(())
}

fn parse_html(bytes: &[u8]) -> Result<ParsedDocument, EngineError> {
    let html = std::str::from_utf8(bytes).map_err(source_error)?;
    validate_html_signature(html)?;
    let mut readability = Readability::new(html, None, None).map_err(source_error)?;
    let article = readability
        .parse()
        .map_err(|error| source_error(format!("HTML article extraction failed: {error}")))?;
    if article.text_content.trim().is_empty() {
        return Err(source_error(
            "HTML has no extractable article text; scripts and images are not rendered",
        ));
    }
    let article_dom = dom_query::Document::from(article.content.as_ref());
    let normalized = article_dom.md(Some(&["script", "style", "noscript", "iframe", "svg"]));
    if normalized.len() > MAX_TEXT_BYTES {
        return Err(EngineError::TooLarge {
            maximum: MAX_TEXT_BYTES,
        });
    }
    let mut document = parse_markdown(normalized.as_bytes())?;
    document.parser = HTML_PARSER.into();
    if !article.title.trim().is_empty() {
        document.title = article.title;
        document.nodes[0].title = document.title.clone();
    }
    document.diagnostics.push("HTML article text normalized to Markdown; offsets refer to this artifact, not raw HTML bytes".into());
    Ok(document)
}

fn validate_pdf_signature(bytes: &[u8]) -> Result<(), EngineError> {
    if !bytes.starts_with(b"%PDF-") {
        return Err(source_error("PDF source is missing its %PDF- signature"));
    }
    Ok(())
}

fn parse_pdf(bytes: &[u8]) -> Result<ParsedDocument, EngineError> {
    validate_pdf_signature(bytes)?;
    let pdf = lopdf::Document::load_mem(bytes).map_err(|error| {
        source_error(format!(
            "PDF load failed (malformed or encrypted document): {error}"
        ))
    })?;
    if pdf.trailer.has(b"Encrypt") || pdf.is_encrypted() || pdf.was_encrypted() {
        return Err(source_error("encrypted PDF sources are not supported"));
    }
    let pages = pdf.get_pages();
    if pages.is_empty() || pages.len() > MAX_PDF_PAGES {
        return Err(source_error(format!(
            "PDF must have 1..={MAX_PDF_PAGES} physical pages"
        )));
    }
    let mut text = String::new();
    let title = "PDF document".to_owned();
    let mut nodes = vec![DocumentNode {
        id: "root".into(),
        parent: None,
        title: title.clone(),
        start: 0,
        end: 0,
        page: None,
    }];
    let mut diagnostics = vec![
        "PDF reading order and table layout are not guaranteed; inspect the original physical page"
            .into(),
    ];
    let mut has_text = false;
    for page in pages.keys().copied() {
        let mut output = LimitedText {
            text: String::new(),
            limit: MAX_TEXT_BYTES.saturating_sub(text.len() + 2),
            overflow: false,
        };
        let result = pdf_extract::output_doc_page(
            &pdf,
            &mut pdf_extract::PlainTextOutput::new(&mut output),
            page,
        );
        if output.overflow || text.len() + output.text.len() + 2 > MAX_TEXT_BYTES {
            return Err(EngineError::TooLarge {
                maximum: MAX_TEXT_BYTES,
            });
        }
        result
            .map_err(|error| source_error(format!("PDF page {page} extraction failed: {error}")))?;
        if output.text.trim().is_empty() {
            diagnostics.push(format!(
                "physical page {page} has no extractable text; OCR may be required"
            ));
        } else {
            has_text = true;
        }
        let start = text.len();
        text.push_str(&output.text);
        // Separators belong to the preceding page, keeping all offsets attributable.
        text.push_str("\n\n");
        nodes.push(DocumentNode {
            id: format!("page-{page:06}"),
            parent: Some("root".into()),
            title: format!("Page {page}"),
            start,
            end: text.len(),
            page: Some(page),
        });
    }
    if !has_text {
        return Err(source_error(
            "PDF contains no extractable text; scanned/image-only documents require OCR",
        ));
    }
    Ok(ParsedDocument {
        schema_version: SOURCE_FORMAT_VERSION,
        parser: PDF_PARSER.into(),
        source_revision: None,
        title,
        text,
        nodes,
        diagnostics,
    })
}

struct LimitedText {
    text: String,
    limit: usize,
    overflow: bool,
}

impl fmt::Write for LimitedText {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if value.len() > self.limit.saturating_sub(self.text.len()) {
            self.overflow = true;
            return Err(fmt::Error);
        }
        self.text.push_str(value);
        Ok(())
    }
}

impl<'a> pdf_extract::ConvertToFmt for &'a mut LimitedText {
    type Writer = &'a mut LimitedText;
    fn convert(self) -> Self::Writer {
        self
    }
}

fn parse_markdown(bytes: &[u8]) -> Result<ParsedDocument, EngineError> {
    let text = String::from_utf8(bytes.to_vec())?;
    if text.len() > MAX_TEXT_BYTES {
        return Err(EngineError::TooLarge {
            maximum: MAX_TEXT_BYTES,
        });
    }
    if text.trim().is_empty() {
        return Err(source_error("source contains no text"));
    }
    let mut headings = Vec::new();
    let mut current: Option<(usize, usize, String)> = None;
    for (event, span) in Parser::new_ext(&text, Options::ENABLE_TABLES | Options::ENABLE_FOOTNOTES)
        .into_offset_iter()
    {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current = Some((span.start, level as usize, String::new()))
            }
            Event::Text(value) | Event::Code(value) => {
                if let Some((_, _, title)) = &mut current {
                    title.push_str(&value);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some((_, _, title)) = &mut current {
                    title.push(' ');
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(heading) = current.take() {
                    headings.push(heading);
                }
            }
            _ => {}
        }
        if headings.len() > MAX_NODES {
            return Err(source_error("too many document headings"));
        }
    }
    let title = headings
        .first()
        .map(|(_, _, title)| title.clone())
        .unwrap_or_else(|| "Untitled document".into());
    let mut nodes = vec![DocumentNode {
        id: "root".into(),
        parent: None,
        title: title.clone(),
        start: 0,
        end: headings.first().map(|h| h.0).unwrap_or(text.len()),
        page: None,
    }];
    let mut parents: Vec<(usize, String)> = vec![(0, "root".into())];
    for (i, (start, level, heading)) in headings.iter().enumerate() {
        while parents
            .last()
            .is_some_and(|(parent_level, _)| parent_level >= level)
        {
            parents.pop();
        }
        let id = format!("n{i:06}");
        nodes.push(DocumentNode {
            id: id.clone(),
            parent: parents.last().map(|(_, id)| id.clone()),
            title: heading.clone(),
            start: *start,
            end: headings.get(i + 1).map(|h| h.0).unwrap_or(text.len()),
            page: None,
        });
        parents.push((*level, id));
    }
    let document = ParsedDocument {
        schema_version: SOURCE_FORMAT_VERSION,
        parser: MARKDOWN_PARSER.into(),
        source_revision: None,
        title,
        text,
        nodes,
        diagnostics: Vec::new(),
    };
    Ok(document)
}
