//! Cross-platform, local document text extraction for desktop and agent tools.
//!
//! The readers in this module do not require Python, Microsoft Office,
//! LibreOffice, or Poppler. They intentionally extract text and basic structure;
//! OCR and pixel-perfect rendering remain separate capabilities.

use anyhow::{bail, Context, Result};
use calamine::{open_workbook_auto, Reader as CalamineReader};
use once_cell::sync::Lazy;
use quick_xml::events::Event;
use quick_xml::Reader as XmlReader;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::path::PathBuf;
use std::sync::RwLock;
use zip::ZipArchive;

pub const DEFAULT_MAX_CHARS: usize = 24_000;
pub const MAX_EXTRACTED_CHARS: usize = 200_000;
const MAX_SOURCE_BYTES: u64 = 100 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 10_000;
const MAX_XML_ENTRY_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SELECTED_XML_BYTES: u64 = 96 * 1024 * 1024;
const MAX_SPREADSHEET_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SPREADSHEET_ENTRY_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SPREADSHEET_RANGE_CELLS: u64 = 5_000_000;
const MAX_SPREADSHEET_ROWS: u64 = 1_048_576;
const MAX_SPREADSHEET_COLUMNS: u64 = 16_384;
const MAX_COMPRESSION_RATIO: u64 = 1_000;

static ALLOWED_DOCUMENT_PATHS: Lazy<RwLock<HashSet<PathBuf>>> =
    Lazy::new(|| RwLock::new(HashSet::new()));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFormat {
    Pdf,
    Word,
    Presentation,
    Spreadsheet,
    Text,
}

impl DocumentFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Word => "word",
            Self::Presentation => "presentation",
            Self::Spreadsheet => "spreadsheet",
            Self::Text => "text",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentExtraction {
    pub format: DocumentFormat,
    pub text: String,
    pub char_count: usize,
    pub truncated: bool,
    pub section_count: usize,
    pub warnings: Vec<String>,
}

impl DocumentExtraction {
    fn from_collector(
        format: DocumentFormat,
        collector: TextCollector,
        section_count: usize,
    ) -> Self {
        let (text, char_count, truncated) = collector.finish();
        let warnings = if text.trim().is_empty() {
            vec![match format {
                DocumentFormat::Pdf => {
                    "No embedded text was found. The PDF may be scanned and require OCR."
                        .to_string()
                }
                _ => "No readable text was found in the document.".to_string(),
            }]
        } else {
            Vec::new()
        };
        Self {
            format,
            text,
            char_count,
            truncated,
            section_count,
            warnings,
        }
    }
}

#[derive(Debug)]
struct TextCollector {
    text: String,
    stored_chars: usize,
    observed_chars: usize,
    max_chars: usize,
    forced_truncated: bool,
}

impl TextCollector {
    fn new(max_chars: usize) -> Self {
        Self {
            text: String::new(),
            stored_chars: 0,
            observed_chars: 0,
            max_chars: max_chars.clamp(1, MAX_EXTRACTED_CHARS),
            forced_truncated: false,
        }
    }

    fn push(&mut self, value: &str) {
        for ch in value.chars().filter(|ch| *ch != '\0') {
            self.observed_chars += 1;
            if self.stored_chars < self.max_chars {
                self.text.push(ch);
                self.stored_chars += 1;
            } else {
                self.forced_truncated = true;
            }
        }
    }

    fn push_line_break(&mut self) {
        if !self.text.ends_with('\n') {
            self.push("\n");
        }
    }

    fn saturated(&self) -> bool {
        self.stored_chars >= self.max_chars
    }

    fn mark_truncated(&mut self) {
        self.forced_truncated = true;
        self.observed_chars = self.observed_chars.max(self.max_chars + 1);
    }

    fn finish(self) -> (String, usize, bool) {
        let text = normalize_text(&self.text);
        (
            text,
            self.observed_chars,
            self.forced_truncated || self.observed_chars > self.stored_chars,
        )
    }
}

pub fn is_supported_document_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(is_supported_document_extension)
}

pub fn is_supported_document_extension(extension: &str) -> bool {
    matches!(
        extension
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str(),
        "pdf"
            | "docx"
            | "docm"
            | "pptx"
            | "pptm"
            | "xlsx"
            | "xlsm"
            | "txt"
            | "md"
            | "markdown"
            | "csv"
            | "tsv"
            | "json"
            | "xml"
            | "yaml"
            | "yml"
    )
}

/// Grants the read-only document tool access to one path selected by the user
/// or registered by a trusted host workflow.
pub fn allow_document_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    let canonical = path
        .as_ref()
        .canonicalize()
        .with_context(|| format!("failed to resolve document {}", path.as_ref().display()))?;
    if !canonical.is_file() {
        bail!("document path is not a file: {}", canonical.display());
    }
    if !is_supported_document_path(&canonical) {
        bail!("unsupported document format: {}", canonical.display());
    }
    ALLOWED_DOCUMENT_PATHS
        .write()
        .map_err(|_| anyhow::anyhow!("document path allowlist lock is poisoned"))?
        .insert(canonical.clone());
    Ok(canonical)
}

pub fn ensure_document_path_allowed(path: impl AsRef<Path>) -> Result<PathBuf> {
    let canonical = path
        .as_ref()
        .canonicalize()
        .with_context(|| format!("failed to resolve document {}", path.as_ref().display()))?;
    let allowed = ALLOWED_DOCUMENT_PATHS
        .read()
        .map_err(|_| anyhow::anyhow!("document path allowlist lock is poisoned"))?
        .contains(&canonical);
    if !allowed {
        bail!(
            "document access was not granted by the user: {}. Upload or import the file first",
            canonical.display()
        );
    }
    Ok(canonical)
}

/// Removes a path from the read-only document capability set.
pub fn revoke_document_path(path: impl AsRef<Path>) -> Result<bool> {
    let candidate = path
        .as_ref()
        .canonicalize()
        .unwrap_or_else(|_| path.as_ref().to_path_buf());
    Ok(ALLOWED_DOCUMENT_PATHS
        .write()
        .map_err(|_| anyhow::anyhow!("document path allowlist lock is poisoned"))?
        .remove(&candidate))
}

pub fn extract_document(path: impl AsRef<Path>, max_chars: usize) -> Result<DocumentExtraction> {
    let path = path.as_ref();
    ensure_source_file_within_limits(path)?;

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => extract_pdf(path, max_chars, None),
        "docx" | "docm" => extract_docx(path, max_chars),
        "pptx" | "pptm" => extract_pptx(path, max_chars),
        "xlsx" | "xlsm" => extract_spreadsheet(path, max_chars),
        "txt" | "md" | "markdown" | "csv" | "tsv" | "json" | "xml" | "yaml" | "yml" => {
            extract_plain_text(path, max_chars)
        }
        "doc" | "ppt" => bail!(
            "legacy .{} files are not supported; save the file as DOCX or PPTX first",
            extension
        ),
        "xls" | "xlsb" | "ods" => bail!(
            ".{} spreadsheets are not supported by the safe desktop reader; save the file as XLSX first",
            extension
        ),
        _ => bail!("unsupported document format: .{}", extension),
    }
}

fn ensure_source_file_within_limits(path: &Path) -> Result<()> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to inspect document {}", path.display()))?;
    if !metadata.is_file() {
        bail!("document path is not a file: {}", path.display());
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        bail!(
            "document is too large ({} bytes, maximum {} bytes)",
            metadata.len(),
            MAX_SOURCE_BYTES
        );
    }
    Ok(())
}

pub fn extract_pdf_pages(
    path: impl AsRef<Path>,
    start_page: u32,
    end_page: Option<u32>,
    max_chars: usize,
) -> Result<DocumentExtraction> {
    extract_pdf(
        path.as_ref(),
        max_chars,
        Some((start_page.max(1), end_page)),
    )
}

fn extract_pdf(
    path: &Path,
    max_chars: usize,
    page_range: Option<(u32, Option<u32>)>,
) -> Result<DocumentExtraction> {
    ensure_source_file_within_limits(path)?;
    let extraction = catch_unwind(AssertUnwindSafe(|| {
        pdf_extract::extract_text_by_pages(path)
    }))
    .map_err(|_| anyhow::anyhow!("PDF parser aborted while reading {}", path.display()))?;
    let pages = extraction.with_context(|| format!("failed to extract PDF {}", path.display()))?;
    let (start_page, end_page) = page_range.unwrap_or((1, None));
    let end_page = end_page.unwrap_or(pages.len() as u32).max(start_page);
    let mut collector = TextCollector::new(max_chars);
    let mut sections = 0usize;

    for (index, page) in pages.iter().enumerate() {
        let page_number = index as u32 + 1;
        if page_number < start_page || page_number > end_page {
            continue;
        }
        sections += 1;
        collector.push(&format!("[Page {page_number}]\n"));
        collector.push(page);
        collector.push_line_break();
        if collector.saturated() && page_number < end_page {
            collector.mark_truncated();
            break;
        }
    }

    Ok(DocumentExtraction::from_collector(
        DocumentFormat::Pdf,
        collector,
        sections,
    ))
}

fn extract_docx(path: &Path, max_chars: usize) -> Result<DocumentExtraction> {
    let mut archive = open_zip(path)?;
    let mut names = archive
        .file_names()
        .filter(|name| is_docx_text_part(name))
        .map(str::to_string)
        .collect::<Vec<_>>();
    names.sort_by_key(|name| docx_part_sort_key(name));
    let mut collector = TextCollector::new(max_chars);
    let mut selected_bytes = 0u64;
    let mut sections = 0usize;

    for name in names {
        if collector.saturated() {
            collector.mark_truncated();
            break;
        }
        let xml = read_zip_text(&mut archive, &name, &mut selected_bytes)?;
        sections += 1;
        if name != "word/document.xml" {
            collector.push(&format!("[{}]\n", friendly_docx_part_name(&name)));
        }
        extract_ooxml_text(&xml, XmlFlavor::Word, &mut collector)?;
        collector.push_line_break();
    }

    Ok(DocumentExtraction::from_collector(
        DocumentFormat::Word,
        collector,
        sections,
    ))
}

fn extract_pptx(path: &Path, max_chars: usize) -> Result<DocumentExtraction> {
    let mut archive = open_zip(path)?;
    let mut selected_bytes = 0u64;
    let slide_names = resolve_pptx_slide_order(&mut archive, &mut selected_bytes)?;
    let slide_count = slide_names.len();
    let mut collector = TextCollector::new(max_chars);
    for (index, name) in slide_names.into_iter().enumerate() {
        if collector.saturated() {
            collector.mark_truncated();
            break;
        }
        let slide_number = index + 1;
        collector.push(&format!("[Slide {slide_number}]\n"));
        let xml = read_zip_text(&mut archive, &name, &mut selected_bytes)?;
        extract_ooxml_text(&xml, XmlFlavor::Presentation, &mut collector)?;
        collector.push_line_break();
        if let Some(notes_name) = resolve_pptx_notes_part(&mut archive, &name, &mut selected_bytes)?
        {
            collector.push(&format!("[Slide {slide_number} Notes]\n"));
            let notes_xml = read_zip_text(&mut archive, &notes_name, &mut selected_bytes)?;
            extract_ooxml_text(&notes_xml, XmlFlavor::Presentation, &mut collector)?;
            collector.push_line_break();
        }
    }

    Ok(DocumentExtraction::from_collector(
        DocumentFormat::Presentation,
        collector,
        slide_count,
    ))
}

fn extract_spreadsheet(path: &Path, max_chars: usize) -> Result<DocumentExtraction> {
    validate_spreadsheet_archive(path)?;
    let mut workbook = open_workbook_auto(path)
        .with_context(|| format!("failed to open spreadsheet {}", path.display()))?;
    let sheet_names = workbook.sheet_names().to_vec();
    let section_count = sheet_names.len();
    let mut collector = TextCollector::new(max_chars);

    for sheet_name in sheet_names {
        if collector.saturated() {
            collector.mark_truncated();
            break;
        }
        let range = workbook
            .worksheet_range(&sheet_name)
            .with_context(|| format!("failed to read worksheet {sheet_name}"))?;
        collector.push(&format!("[Sheet: {sheet_name}]\n"));
        for row in range.rows() {
            let mut values = row.iter().map(ToString::to_string).collect::<Vec<_>>();
            while values.last().is_some_and(|value| value.trim().is_empty()) {
                values.pop();
            }
            if !values.is_empty() {
                collector.push(&values.join("\t"));
                collector.push_line_break();
            }
            if collector.saturated() {
                collector.mark_truncated();
                break;
            }
        }
    }

    Ok(DocumentExtraction::from_collector(
        DocumentFormat::Spreadsheet,
        collector,
        section_count,
    ))
}

fn extract_plain_text(path: &Path, max_chars: usize) -> Result<DocumentExtraction> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let mut collector = TextCollector::new(max_chars);
    collector.push(&text);
    Ok(DocumentExtraction::from_collector(
        DocumentFormat::Text,
        collector,
        1,
    ))
}

fn open_zip(path: &Path) -> Result<ZipArchive<File>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let archive = ZipArchive::new(file)
        .with_context(|| format!("failed to parse OOXML package {}", path.display()))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        bail!(
            "OOXML package contains too many entries ({}; maximum {})",
            archive.len(),
            MAX_ARCHIVE_ENTRIES
        );
    }
    Ok(archive)
}

fn validate_spreadsheet_archive(path: &Path) -> Result<()> {
    let mut archive = open_zip(path)?;
    let mut expanded_bytes = 0u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .with_context(|| format!("failed to inspect spreadsheet archive entry {index}"))?;
        if entry.size() > MAX_SPREADSHEET_ENTRY_BYTES {
            bail!(
                "spreadsheet archive entry {} is too large ({} bytes)",
                entry.name(),
                entry.size()
            );
        }
        if entry.size() > 1024 * 1024
            && entry.compressed_size() > 0
            && entry.size() / entry.compressed_size() > MAX_COMPRESSION_RATIO
        {
            bail!(
                "spreadsheet archive entry {} has an unsafe compression ratio",
                entry.name()
            );
        }
        expanded_bytes = expanded_bytes.saturating_add(entry.size());
        if expanded_bytes > MAX_SPREADSHEET_ARCHIVE_BYTES {
            bail!("spreadsheet archive exceeds the safe expansion limit");
        }
    }

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "xlsx" | "xlsm") {
        let worksheet_names = archive
            .file_names()
            .filter(|name| name.starts_with("xl/worksheets/") && name.ends_with(".xml"))
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut selected_bytes = 0u64;
        for name in worksheet_names {
            let xml = read_zip_text(&mut archive, &name, &mut selected_bytes)?;
            validate_xlsx_worksheet_dimensions(&xml, &name)?;
        }
    }
    Ok(())
}

fn validate_xlsx_worksheet_dimensions(xml: &str, name: &str) -> Result<()> {
    let mut reader = XmlReader::from_str(xml);
    let mut bounds: Option<(u64, u64, u64, u64)> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event) | Event::Empty(event)) => {
                let local_name = event.local_name();
                if local_name.as_ref() != b"c" && local_name.as_ref() != b"dimension" {
                    continue;
                }
                for attribute in event.attributes() {
                    let attribute = attribute.context("failed to read XLSX cell reference")?;
                    if attribute.key.as_ref() != b"r" && attribute.key.as_ref() != b"ref" {
                        continue;
                    }
                    let reference = attribute
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )
                        .context("failed to decode XLSX cell reference")?;
                    update_spreadsheet_bounds(&mut bounds, &reference, name)?;
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(error).with_context(|| format!("failed to parse worksheet {name}"));
            }
            _ => {}
        }
    }
    Ok(())
}

fn update_spreadsheet_bounds(
    bounds: &mut Option<(u64, u64, u64, u64)>,
    reference: &str,
    name: &str,
) -> Result<()> {
    let mut endpoints = reference.split(':');
    let first = parse_xlsx_cell_reference(endpoints.next().unwrap_or_default())
        .with_context(|| format!("invalid worksheet reference {reference:?} in {name}"))?;
    let last = match endpoints.next() {
        Some(value) => parse_xlsx_cell_reference(value)
            .with_context(|| format!("invalid worksheet reference {reference:?} in {name}"))?,
        None => first,
    };
    if endpoints.next().is_some() {
        bail!("invalid worksheet range {reference:?} in {name}");
    }
    for (row, column) in [first, last] {
        if row >= MAX_SPREADSHEET_ROWS || column >= MAX_SPREADSHEET_COLUMNS {
            bail!("worksheet reference {reference:?} exceeds Excel limits in {name}");
        }
        *bounds = Some(match *bounds {
            Some((min_row, min_column, max_row, max_column)) => (
                min_row.min(row),
                min_column.min(column),
                max_row.max(row),
                max_column.max(column),
            ),
            None => (row, column, row, column),
        });
    }
    let (min_row, min_column, max_row, max_column) = bounds.as_ref().copied().unwrap_or_default();
    let height = max_row.saturating_sub(min_row).saturating_add(1);
    let width = max_column.saturating_sub(min_column).saturating_add(1);
    if height.saturating_mul(width) > MAX_SPREADSHEET_RANGE_CELLS {
        bail!(
            "worksheet {name} declares a range of {} cells, above the safe limit of {}",
            height.saturating_mul(width),
            MAX_SPREADSHEET_RANGE_CELLS
        );
    }
    Ok(())
}

fn parse_xlsx_cell_reference(reference: &str) -> Result<(u64, u64)> {
    let mut column = 0u64;
    let mut row_digits = String::new();
    let mut saw_column = false;
    for character in reference
        .trim()
        .chars()
        .filter(|character| *character != '$')
    {
        if character.is_ascii_alphabetic() && row_digits.is_empty() {
            saw_column = true;
            column = column
                .checked_mul(26)
                .and_then(|value| {
                    value.checked_add(character.to_ascii_uppercase() as u64 - b'A' as u64 + 1)
                })
                .ok_or_else(|| anyhow::anyhow!("column index overflow"))?;
        } else if character.is_ascii_digit() && saw_column {
            row_digits.push(character);
        } else {
            bail!("unsupported cell reference {reference:?}");
        }
    }
    if !saw_column || column == 0 || row_digits.is_empty() {
        bail!("incomplete cell reference {reference:?}");
    }
    let row = row_digits
        .parse::<u64>()
        .context("row index is not a number")?;
    if row == 0 {
        bail!("row indexes start at one");
    }
    Ok((row - 1, column - 1))
}

fn resolve_pptx_slide_order(
    archive: &mut ZipArchive<File>,
    selected_bytes: &mut u64,
) -> Result<Vec<String>> {
    let mut fallback = archive
        .file_names()
        .filter(|name| is_numbered_xml_part(name, "ppt/slides/slide"))
        .map(str::to_string)
        .collect::<Vec<_>>();
    fallback.sort_by_key(|name| numbered_part_index(name));

    const PRESENTATION: &str = "ppt/presentation.xml";
    const RELATIONSHIPS: &str = "ppt/_rels/presentation.xml.rels";
    if archive.index_for_name(PRESENTATION).is_none()
        || archive.index_for_name(RELATIONSHIPS).is_none()
    {
        return Ok(fallback);
    }

    let presentation = read_zip_text(archive, PRESENTATION, selected_bytes)?;
    let relationship_ids = presentation_slide_relationship_ids(&presentation)?;
    let relationships = read_zip_text(archive, RELATIONSHIPS, selected_bytes)?;
    let targets = relationship_targets(&relationships)?
        .into_iter()
        .filter(|(_, _, relation_type)| relation_type.ends_with("/slide"))
        .map(|(id, target, _)| (id, target))
        .collect::<HashMap<_, _>>();
    let ordered = relationship_ids
        .iter()
        .filter_map(|id| targets.get(id))
        .filter_map(|target| normalize_presentation_slide_target(target))
        .filter(|name| archive.index_for_name(name).is_some())
        .collect::<Vec<_>>();

    if !relationship_ids.is_empty() && ordered.len() == relationship_ids.len() {
        Ok(ordered)
    } else {
        Ok(fallback)
    }
}

fn resolve_pptx_notes_part(
    archive: &mut ZipArchive<File>,
    slide_name: &str,
    selected_bytes: &mut u64,
) -> Result<Option<String>> {
    let Some(file_name) = Path::new(slide_name)
        .file_name()
        .and_then(|value| value.to_str())
    else {
        return Ok(None);
    };
    let relationships_name = format!("ppt/slides/_rels/{file_name}.rels");
    if archive.index_for_name(&relationships_name).is_none() {
        return Ok(None);
    }
    let relationships = read_zip_text(archive, &relationships_name, selected_bytes)?;
    for (_, target, relation_type) in relationship_targets(&relationships)? {
        if !relation_type.ends_with("/notesSlide") {
            continue;
        }
        let Some(name) = normalize_slide_notes_target(&target) else {
            continue;
        };
        if archive.index_for_name(&name).is_some() {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

fn presentation_slide_relationship_ids(xml: &str) -> Result<Vec<String>> {
    let mut reader = XmlReader::from_str(xml);
    let mut ids = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event) | Event::Empty(event))
                if event.local_name().as_ref() == b"sldId" =>
            {
                for attribute in event.attributes() {
                    let attribute = attribute.context("failed to read PPTX slide attribute")?;
                    let key = attribute.key.as_ref();
                    if key == b"r:id" || key.ends_with(b":id") {
                        ids.push(
                            attribute
                                .decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .context("failed to decode PPTX slide relationship id")?
                                .into_owned(),
                        );
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(error).context("failed to parse PPTX presentation order"),
            _ => {}
        }
    }
    Ok(ids)
}

fn relationship_targets(xml: &str) -> Result<Vec<(String, String, String)>> {
    let mut reader = XmlReader::from_str(xml);
    let mut relationships = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event) | Event::Empty(event))
                if event.local_name().as_ref() == b"Relationship" =>
            {
                let mut id = String::new();
                let mut target = String::new();
                let mut relation_type = String::new();
                for attribute in event.attributes() {
                    let attribute = attribute.context("failed to read PPTX relationship")?;
                    let value = attribute
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )
                        .context("failed to decode PPTX relationship")?
                        .into_owned();
                    match attribute.key.as_ref() {
                        b"Id" => id = value,
                        b"Target" => target = value,
                        b"Type" => relation_type = value,
                        _ => {}
                    }
                }
                if !id.is_empty() && !target.is_empty() {
                    relationships.push((id, target, relation_type));
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(error).context("failed to parse PPTX relationships"),
            _ => {}
        }
    }
    Ok(relationships)
}

fn normalize_presentation_slide_target(target: &str) -> Option<String> {
    let normalized = target.trim().replace('\\', "/");
    let candidate = if let Some(relative) = normalized.strip_prefix("slides/") {
        format!("ppt/slides/{relative}")
    } else {
        normalized.trim_start_matches('/').to_string()
    };
    is_numbered_xml_part(&candidate, "ppt/slides/slide").then_some(candidate)
}

fn normalize_slide_notes_target(target: &str) -> Option<String> {
    let normalized = target.trim().replace('\\', "/");
    let relative = normalized
        .strip_prefix("../notesSlides/")
        .or_else(|| normalized.strip_prefix("notesSlides/"))?;
    let candidate = format!("ppt/notesSlides/{relative}");
    is_numbered_xml_part(&candidate, "ppt/notesSlides/notesSlide").then_some(candidate)
}

fn read_zip_text(
    archive: &mut ZipArchive<File>,
    name: &str,
    selected_bytes: &mut u64,
) -> Result<String> {
    let mut entry = archive
        .by_name(name)
        .with_context(|| format!("missing OOXML part {name}"))?;
    if entry.size() > MAX_XML_ENTRY_BYTES {
        bail!("OOXML part {name} is too large ({} bytes)", entry.size());
    }
    if entry.size() > 1024 * 1024
        && entry.compressed_size() > 0
        && entry.size() / entry.compressed_size() > MAX_COMPRESSION_RATIO
    {
        bail!("OOXML part {name} has an unsafe compression ratio");
    }
    *selected_bytes = selected_bytes.saturating_add(entry.size());
    if *selected_bytes > MAX_SELECTED_XML_BYTES {
        bail!("selected OOXML content exceeds the safe extraction limit");
    }
    let mut bytes = Vec::with_capacity(entry.size().min(1024 * 1024) as usize);
    entry
        .by_ref()
        .take(MAX_XML_ENTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read OOXML part {name}"))?;
    if bytes.len() as u64 > MAX_XML_ENTRY_BYTES {
        bail!("OOXML part {name} expanded beyond the safe extraction limit");
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[derive(Debug, Clone, Copy)]
enum XmlFlavor {
    Word,
    Presentation,
}

fn extract_ooxml_text(xml: &str, flavor: XmlFlavor, collector: &mut TextCollector) -> Result<()> {
    let mut reader = XmlReader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut inside_text = 0usize;

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) if event.local_name().as_ref() == b"t" => {
                inside_text += 1;
            }
            Ok(Event::End(event)) if event.local_name().as_ref() == b"t" => {
                inside_text = inside_text.saturating_sub(1);
            }
            Ok(Event::Text(event)) if inside_text > 0 => {
                let decoded = event.decode().context("failed to decode OOXML text")?;
                collector.push(&decoded);
            }
            Ok(Event::CData(event)) if inside_text > 0 => {
                collector.push(&event.decode().context("failed to decode OOXML CDATA")?);
            }
            Ok(Event::GeneralRef(event)) if inside_text > 0 => {
                if let Some(character) = event
                    .resolve_char_ref()
                    .context("failed to decode OOXML character reference")?
                {
                    collector.push(&character.to_string());
                } else {
                    let name = event.decode().context("failed to decode OOXML entity")?;
                    let entity = match name.as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "apos" => "'",
                        "quot" => "\"",
                        _ => "",
                    };
                    collector.push(entity);
                }
            }
            Ok(Event::Empty(event)) => {
                let local = event.local_name();
                if matches!(flavor, XmlFlavor::Word) && local.as_ref() == b"tab" {
                    collector.push("\t");
                } else if local.as_ref() == b"br" {
                    collector.push_line_break();
                }
            }
            Ok(Event::End(event)) => {
                let local = event.local_name();
                if local.as_ref() == b"p" || local.as_ref() == b"tr" {
                    collector.push_line_break();
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(error).context("failed to parse OOXML XML"),
            _ => {}
        }
        if collector.saturated() {
            collector.mark_truncated();
            break;
        }
    }
    Ok(())
}

fn is_docx_text_part(name: &str) -> bool {
    name == "word/document.xml"
        || ((name.starts_with("word/header") || name.starts_with("word/footer"))
            && name.ends_with(".xml"))
        || matches!(
            name,
            "word/footnotes.xml"
                | "word/endnotes.xml"
                | "word/comments.xml"
                | "word/glossary/document.xml"
        )
}

fn docx_part_sort_key(name: &str) -> (u8, u32, String) {
    let rank = if name == "word/document.xml" {
        0
    } else if name.starts_with("word/header") {
        1
    } else if name.starts_with("word/footer") {
        2
    } else if name == "word/footnotes.xml" {
        3
    } else if name == "word/endnotes.xml" {
        4
    } else {
        5
    };
    (rank, numbered_part_index(name), name.to_string())
}

fn friendly_docx_part_name(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("document part")
        .to_string()
}

fn is_numbered_xml_part(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(".xml"))
        .is_some_and(|number| !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit()))
}

fn numbered_part_index(name: &str) -> u32 {
    Path::new(name)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(|value| {
            value
                .chars()
                .rev()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        })
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(u32::MAX)
}

fn normalize_text(text: &str) -> String {
    let mut output = String::new();
    let mut blank_lines = 0usize;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank_lines += 1;
            if blank_lines > 1 {
                continue;
            }
        } else {
            blank_lines = 0;
        }
        output.push_str(line);
        output.push('\n');
    }
    output.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn write_zip_fixture(path: &Path, entries: &[(&str, &str)]) {
        let mut bytes = Vec::new();
        {
            let mut writer = ZipWriter::new(Cursor::new(&mut bytes));
            for (name, content) in entries {
                writer
                    .start_file(*name, SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(content.as_bytes()).unwrap();
            }
            writer.finish().unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn extracts_docx_paragraphs_and_table_cells() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("sample.docx");
        write_zip_fixture(
            &path,
            &[(
                "word/document.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?>
                <w:document xmlns:w="urn:w"><w:body>
                  <w:p><w:r><w:t>季度报告 &amp; 计划</w:t></w:r></w:p>
                  <w:tbl><w:tr><w:tc><w:p><w:r><w:t>收入</w:t></w:r></w:p></w:tc>
                  <w:tc><w:p><w:r><w:t>100</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
                </w:body></w:document>"#,
            )],
        );

        let result = extract_document(&path, 10_000).unwrap();
        assert_eq!(result.format, DocumentFormat::Word);
        assert!(
            result.text.contains("季度报告 & 计划"),
            "unexpected DOCX text: {:?}",
            result.text
        );
        assert!(result.text.contains("收入"));
        assert!(result.text.contains("100"));
    }

    #[test]
    fn extracts_pptx_in_presentation_order_with_matching_notes() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("sample.pptx");
        write_zip_fixture(
            &path,
            &[
                (
                    "ppt/slides/slide10.xml",
                    r#"<p:sld xmlns:p="p" xmlns:a="a"><a:p><a:r><a:t>第十页</a:t></a:r></a:p></p:sld>"#,
                ),
                (
                    "ppt/slides/slide2.xml",
                    r#"<p:sld xmlns:p="p" xmlns:a="a"><a:p><a:r><a:t>第二页</a:t></a:r></a:p></p:sld>"#,
                ),
                (
                    "ppt/slides/slide1.xml",
                    r#"<p:sld xmlns:p="p" xmlns:a="a"><a:p><a:r><a:t>标题页</a:t></a:r></a:p></p:sld>"#,
                ),
                (
                    "ppt/presentation.xml",
                    r#"<p:presentation xmlns:p="p" xmlns:r="r"><p:sldIdLst><p:sldId r:id="rId2"/><p:sldId r:id="rId10"/><p:sldId r:id="rId1"/></p:sldIdLst></p:presentation>"#,
                ),
                (
                    "ppt/_rels/presentation.xml.rels",
                    r#"<Relationships><Relationship Id="rId1" Type="office/slide" Target="slides/slide1.xml"/><Relationship Id="rId2" Type="office/slide" Target="slides/slide2.xml"/><Relationship Id="rId10" Type="office/slide" Target="slides/slide10.xml"/></Relationships>"#,
                ),
                (
                    "ppt/slides/_rels/slide2.xml.rels",
                    r#"<Relationships><Relationship Id="notes" Type="office/notesSlide" Target="../notesSlides/notesSlide7.xml"/></Relationships>"#,
                ),
                (
                    "ppt/notesSlides/notesSlide7.xml",
                    r#"<p:notes xmlns:p="p" xmlns:a="a"><a:p><a:r><a:t>第二页讲者备注</a:t></a:r></a:p></p:notes>"#,
                ),
            ],
        );

        let result = extract_document(&path, 10_000).unwrap();
        assert_eq!(result.format, DocumentFormat::Presentation);
        assert_eq!(result.section_count, 3);
        assert!(result.text.find("第二页").unwrap() < result.text.find("第十页").unwrap());
        assert!(result.text.find("第十页").unwrap() < result.text.find("标题页").unwrap());
        assert!(result.text.find("第二页").unwrap() < result.text.find("第二页讲者备注").unwrap());
        assert!(result.text.contains("[Slide 1 Notes]"));
    }

    #[test]
    fn reads_existing_xlsx_fixture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../goose-mcp/src/computercontroller/tests/data/FinancialSample.xlsx");
        let result = extract_document(path, 20_000).unwrap();
        assert_eq!(result.format, DocumentFormat::Spreadsheet);
        assert!(result.text.contains("Government"));
        assert!(result.section_count >= 1);
    }

    #[test]
    fn rejects_spreadsheet_archive_with_unsafe_compression_ratio() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("compressed-bomb.xlsx");
        let mut bytes = Vec::new();
        {
            let mut writer = ZipWriter::new(Cursor::new(&mut bytes));
            writer
                .start_file(
                    "xl/worksheets/sheet1.xml",
                    SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(&vec![b'0'; 4 * 1024 * 1024]).unwrap();
            writer.finish().unwrap();
        }
        fs::write(&path, bytes).unwrap();

        let error = extract_document(path, 1_000).unwrap_err().to_string();
        assert!(error.contains("unsafe compression ratio"));
    }

    #[test]
    fn rejects_xlsx_with_a_dangerously_sparse_declared_range() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("sparse-range.xlsx");
        write_zip_fixture(
            &path,
            &[(
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><dimension ref="A1:XFD1048576"/><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData></worksheet>"#,
            )],
        );

        let error = extract_document(path, 1_000).unwrap_err().to_string();
        assert!(error.contains("above the safe limit"));
    }

    #[test]
    fn reads_existing_pdf_fixture_without_external_commands() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../goose-mcp/src/computercontroller/tests/data/test.pdf");
        let result = extract_document(path, 20_000).unwrap();
        assert_eq!(result.format, DocumentFormat::Pdf);
        assert!(!result.text.trim().is_empty());
        assert!(result.section_count >= 1);
    }

    #[test]
    fn pdf_page_reader_rejects_oversized_input_before_parsing() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("oversized.pdf");
        File::create(&path)
            .unwrap()
            .set_len(MAX_SOURCE_BYTES + 1)
            .unwrap();

        let error = extract_pdf_pages(path, 1, None, 1_000)
            .unwrap_err()
            .to_string();
        assert!(error.contains("document is too large"));
    }

    #[test]
    fn truncates_without_splitting_unicode() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("sample.txt");
        fs::write(&path, "甲乙丙丁戊").unwrap();
        let result = extract_document(path, 3).unwrap();
        assert_eq!(result.text, "甲乙丙");
        assert!(result.truncated);
        assert_eq!(result.char_count, 5);
    }

    #[test]
    fn rejects_legacy_powerpoint_with_actionable_error() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("legacy.ppt");
        fs::write(&path, b"legacy").unwrap();
        let error = extract_document(path, 100).unwrap_err().to_string();
        assert!(error.contains("save the file as DOCX or PPTX"));
    }

    #[test]
    fn rejects_legacy_spreadsheet_with_actionable_error() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("legacy.xls");
        fs::write(&path, b"legacy").unwrap();
        let error = extract_document(path, 100).unwrap_err().to_string();
        assert!(error.contains("save the file as XLSX"));
    }

    #[test]
    fn revoked_document_path_is_no_longer_readable_by_the_tool() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("private.txt");
        fs::write(&path, "private").unwrap();

        allow_document_path(&path).unwrap();
        assert!(ensure_document_path_allowed(&path).is_ok());
        assert!(revoke_document_path(&path).unwrap());
        assert!(ensure_document_path_allowed(&path).is_err());
    }
}
