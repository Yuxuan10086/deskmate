//! 课件解析：pptx（quick-xml）+ pdf（pdfium 预留）。
//! 解析目标：提取章节标题、关键词、页数/幻灯片数，用于自动归类到对应课堂。

use std::fs::File;
use std::io::{BufReader, Read};
use std::ops::Deref;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use zip::read::ZipArchive;

use crate::error::AppResult;

/// 单页幻灯片里的文本节点。
#[derive(Debug, Default)]
pub struct Slide {
    pub index: usize,
    pub texts: Vec<String>,
}

/// 解析结果：课件概览 + 每页文本。
#[derive(Debug)]
pub struct ParsedMaterial {
    pub file_type: String,
    pub page_count: usize,
    pub slides: Vec<Slide>,
    pub keywords: Vec<String>,
    pub confidence: String, // high / low / manual
}

/* --------------------------------- pptx --------------------------------- */

pub fn parse_pptx(path: &Path) -> AppResult<ParsedMaterial> {
    let file = File::open(path)?;
    let mut zip = ZipArchive::new(BufReader::new(file))?;

    // 直接数 ppt/slides/slide*.xml
    let slide_names: Vec<String> = (1..=1000)
        .map(|i| format!("ppt/slides/slide{}.xml", i))
        .take_while(|name| zip.by_name(name).is_ok())
        .collect();

    let mut slides = Vec::with_capacity(slide_names.len());
    let mut all_text = String::new();

    for (idx, name) in slide_names.iter().enumerate() {
        if let Ok(entry) = zip.by_name(name) {
            let slide = extract_slide_text(BufReader::new(entry), idx + 1)?;
            all_text.push_str(&slide.texts.join(" "));
            if !slide.texts.is_empty() {
                slides.push(slide);
            }
        }
    }

    let total = slides.len();
    let keywords = extract_keywords(&all_text, 6);
    let confidence = if total > 0 { "high" } else { "low" };

    Ok(ParsedMaterial {
        file_type: "pptx".into(),
        page_count: total,
        slides,
        keywords,
        confidence: confidence.into(),
    })
}

fn extract_slide_text<R: Read>(input: R, index: usize) -> AppResult<Slide> {
    // 先把输入读成 Vec<u8>，再用 from_slice 构造 Reader（quick-xml 的 Reader<&[u8]> API 最全）
    let mut bytes = Vec::new();
    let mut br = BufReader::new(input);
    br.read_to_end(&mut bytes)?;

    let mut reader: Reader<&[u8]> = Reader::from_reader(bytes.as_slice());
    // quick-xml 0.37 默认不 trim 文本；下面自己做 .trim() 即可

    let mut out = Slide {
        index,
        texts: Vec::new(),
    };
    loop {
        match reader.read_event()? {
            Event::Text(e) => {
                let raw: &[u8] = e.deref();
                let s = String::from_utf8_lossy(raw);
                let trimmed = s.trim().to_string();
                if !trimmed.is_empty() {
                    out.texts.push(trimmed);
                }
            }
            Event::CData(e) => {
                let raw: &[u8] = e.deref();
                let s = String::from_utf8_lossy(raw);
                let trimmed = s.trim().to_string();
                if !trimmed.is_empty() {
                    out.texts.push(trimmed);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

/* --------------------------------- pdf --------------------------------- */

pub fn parse_pdf(path: &Path) -> AppResult<ParsedMaterial> {
    // pdfium 方案：渲染第一页成位图用于封面 + 提取每页纯文本。
    // 当前为轻量占位实现（依赖后续引入 pdfium 或纯文本提取）。
    let file = File::open(path)?;
    let _ = file.metadata()?.len();

    Ok(ParsedMaterial {
        file_type: "pdf".into(),
        page_count: 0,
        slides: Vec::new(),
        keywords: Vec::new(),
        confidence: "low".into(),
    })
}

/* --------------------------------- 关键词 ------------------------------- */

/// 非常朴素的关键词提取：英文词（>=2 字母）+ 中文 2-gram，各按频率排序取前 N。
fn extract_keywords(text: &str, n: usize) -> Vec<String> {
    use std::collections::HashMap;
    let mut freq: HashMap<String, usize> = HashMap::new();

    // 英文词：按非字母切分、小写、>=2 字符
    for word in text.split(|c: char| !(c.is_ascii_alphabetic() || c == '\'' || c == '-')) {
        let w = word.trim().to_lowercase();
        if w.len() >= 2 && !w.chars().all(|c| c == '\'' || c == '-') {
            *freq.entry(w).or_insert(0) += 1;
        }
    }

    // 中文 2-gram：CJK Unified Ideographs 基本区 + 扩展 A
    let is_cjk = |c: char| {
        (c as u32) >= 0x4E00 && (c as u32) <= 0x9FFF
            || (c as u32) >= 0x3400 && (c as u32) <= 0x4DBF
    };
    let chars: Vec<char> = text.chars().filter(|c| is_cjk(*c)).collect();
    for chunk in chars.windows(2) {
        let s: String = chunk.iter().collect();
        if !s.is_empty() {
            *freq.entry(s).or_insert(0) += 1;
        }
    }

    let mut pairs: Vec<(String, usize)> = freq.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1));
    pairs.into_iter().take(n).map(|(k, _)| k).collect()
}

/* --------------------------------- 路由 --------------------------------- */

pub fn parse_material(path: &Path) -> AppResult<ParsedMaterial> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("pptx") | Some("ppt") => parse_pptx(path),
        Some("pdf") => parse_pdf(path),
        _ => Err(crate::error::AppError::new("不支持的课件格式")),
    }
}
