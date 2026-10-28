use crate::rich_content::RichDocument;
use dom_smoothie::{Config, Readability};
use sha2::{Digest, Sha256};
#[derive(serde::Serialize)]
pub struct ExtractionEvidence {
    pub input_sha256: String,
    pub input_bytes: usize,
    pub source_url: String,
    pub extracted_html_sha256: String,
    pub title: String,
    pub word_count: usize,
    pub ending_verified: bool,
    pub structure_image_count: usize,
}
pub fn extract_with_evidence(
    input: &str,
    url: &str,
) -> Result<(RichDocument, ExtractionEvidence), String> {
    if input.len() > 5_000_000 {
        return Err("Source response exceeds 5 MB".into());
    }
    let config = Config {
        max_elements_to_parse: 100_000,
        ..Default::default()
    };
    let mut extractor =
        Readability::new(input, Some(url), Some(config)).map_err(|e| e.to_string())?;
    let article = extractor.parse().map_err(|e| e.to_string())?;
    let document = RichDocument::parse(&article.content);
    let source = scraper::Html::parse_document(input);
    let source_root = source
        .select(
            &scraper::Selector::parse("[itemprop=articleBody], article").expect("static selector"),
        )
        .next();
    let source_ending = source_root
        .and_then(|r| {
            r.select(&scraper::Selector::parse("p").expect("static selector"))
                .last()
        })
        .map(|p| p.text().collect::<String>())
        .unwrap_or_default();
    let text = document.plain_text();
    let evidence = ExtractionEvidence {
        input_sha256: format!("{:x}", Sha256::digest(input.as_bytes())),
        input_bytes: input.len(),
        source_url: url.into(),
        extracted_html_sha256: format!("{:x}", Sha256::digest(article.content.as_bytes())),
        title: article.title,
        word_count: text.split_whitespace().count(),
        ending_verified: !source_ending.is_empty() && text.contains(&source_ending),
        structure_image_count: document.image_sources().len(),
    };
    Ok((document, evidence))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_complete_source_ending_and_structure() {
        let mut input = String::from(
            "<html><head><title>Retained proof</title></head><body><nav>Navigation</nav><article><h1>Retained proof</h1><p>This is meaningful source prose about native applications, retrieval evidence, and correct extraction. It has enough descriptive content to be treated as a useful article, rather than a short sidebar or menu.</p><figure><img src='https://example.org/image.png'><figcaption>Original caption</figcaption></figure><table><tr><th>Result</th><th>Evidence</th></tr><tr><td>Complete</td><td>Verified</td></tr></table>",
        );
        for _ in 0..8 {
            input.push_str("<p>Every complete source paragraph is retained with its words and publication provenance. Tables are substantive article structures. A useful article cannot silently lose the last prose paragraph.</p>");
        }
        input.push_str("<p>Original prose ending must survive.</p></article><footer>Unrelated footer</footer></body></html>");
        let (doc, evidence) = extract_with_evidence(&input, "https://example.org/article").unwrap();
        assert!(evidence.ending_verified);
        let text = doc.plain_text();
        assert!(text.contains("Verified"));
        assert!(text.contains("Original caption"));
        assert_eq!(evidence.structure_image_count, 1);
        assert!(!text.contains("Navigation"));
    }
}
