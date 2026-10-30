//! Article extraction and retained-copy rules. All I/O belongs to the intake owner.
use crate::domain::CopyStatus;
use dom_smoothie::{Config, Readability};
use scraper::{ElementRef, Html, Selector};
use sha2::{Digest, Sha256};
use url::Url;

#[derive(Clone, Debug)]
pub struct Extraction {
    pub title: String,
    pub author: Option<String>,
    pub site: String,
    pub html: String,
    pub word_count: u32,
    pub excerpt: String,
    pub status: CopyStatus,
    pub error: Option<String>,
    pub published_at: Option<String>,
    pub logo_sources: Vec<String>,
    pub source_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetentionDecision {
    Replace,
    Keep { verified: bool, diagnostic: String },
}
fn select(css: &str) -> Selector {
    Selector::parse(css).expect("static selector")
}
fn normalized(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn compact(value: &str) -> String {
    value.chars().filter(|c| !c.is_whitespace()).collect()
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn excluded(e: ElementRef<'_>) -> bool {
    e.ancestors()
        .filter_map(ElementRef::wrap)
        .chain(std::iter::once(e))
        .any(|e| {
            matches!(
                e.value().name(),
                "aside" | "nav" | "footer" | "form" | "script" | "style"
            ) || e.value().attr("hidden").is_some()
                || e.value().attr("aria-hidden") == Some("true")
                || e.value().attr("style").is_some_and(|s| {
                    let s = s.replace(' ', "").to_lowercase();
                    s.contains("display:none") || s.contains("visibility:hidden")
                })
        })
}
fn footnote(e: ElementRef<'_>) -> bool {
    e.ancestors().filter_map(ElementRef::wrap).any(|e| {
        e.value().attr("class").is_some_and(|c| {
            c.split_whitespace()
                .any(|v| matches!(v, "footnote" | "footnotes"))
        }) || matches!(
            e.value().attr("role"),
            Some("doc-footnote" | "doc-endnotes")
        ) || e.value().name() == "figcaption"
    })
}
fn restricted(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "subscribe to read",
        "subscribe now to read",
        "subscribe to continue",
        "subscribe to unlock",
        "sign in to read",
        "log in to read",
        "sign in to continue",
        "log in to continue",
        "sign in to view",
        "article is only for subscribers",
        "story is only for subscribers",
        "post is only for paid",
        "enable javascript",
        "verify that you are human",
        "verify you are human",
        "access denied",
    ]
    .iter()
    .any(|gate| text.contains(gate))
}
fn json_evidence(value: &serde_json::Value, depth: usize, restricted: &mut bool, words: &mut u64) {
    if depth > 15 {
        return;
    }
    match value {
        serde_json::Value::Object(object) => {
            let article = object.get("@type").is_some_and(|t| match t {
                serde_json::Value::String(s) => ["Article", "BlogPosting", "Report"]
                    .iter()
                    .any(|v| s.contains(v)),
                serde_json::Value::Array(a) => a.iter().any(|v| {
                    v.as_str().is_some_and(|s| {
                        ["Article", "BlogPosting", "Report"]
                            .iter()
                            .any(|v| s.contains(v))
                    })
                }),
                _ => false,
            });
            if article {
                if object
                    .get("isAccessibleForFree")
                    .is_some_and(|v| v == false || v == "false")
                {
                    *restricted = true;
                }
                if let Some(v) = object.get("wordCount") {
                    let count = v
                        .as_u64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                        .unwrap_or(0);
                    *words = (*words).max(count);
                }
            }
            for v in object.values() {
                json_evidence(v, depth + 1, restricted, words);
            }
        }
        serde_json::Value::Array(array) => {
            for v in array {
                json_evidence(v, depth + 1, restricted, words);
            }
        }
        _ => {}
    }
}
/// Sanitize source HTML while preserving semantic article structures. Remote raster
/// URLs are admitted only before intake replaces them with private cached media IDs.
pub fn clean_html(html: &str, base: &str, remote_images: bool) -> String {
    fn emit(element: ElementRef<'_>, base: Option<&Url>, remote: bool, out: &mut String) {
        if excluded(element) {
            return;
        }
        let tag = element.value().name();
        let allowed = matches!(
            tag,
            "p" | "br"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "strong"
                | "em"
                | "b"
                | "i"
                | "u"
                | "s"
                | "blockquote"
                | "ul"
                | "ol"
                | "li"
                | "a"
                | "pre"
                | "code"
                | "hr"
                | "img"
                | "figure"
                | "figcaption"
                | "table"
                | "colgroup"
                | "col"
                | "caption"
                | "tfoot"
                | "thead"
                | "tbody"
                | "tr"
                | "th"
                | "td"
                | "sup"
                | "sub"
                | "details"
                | "summary"
        );
        let mut attrs = Vec::<(&str, String)>::new();
        let valid_anchor = |value: &str| {
            !value.is_empty()
                && value.len() <= 256
                && !value.chars().any(|c| c.is_control() || c.is_whitespace())
        };
        if allowed
            && let Some(id) = element
                .value()
                .attr("id")
                .or_else(|| (tag == "a").then(|| element.value().attr("name")).flatten())
                .filter(|id| valid_anchor(id))
        {
            attrs.push(("id", id.to_owned()));
        }
        if tag == "a"
            && let Some(href) = element
                .value()
                .attr("href")
                .and_then(|h| {
                    base.and_then(|base| base.join(h).ok())
                        .or_else(|| Url::parse(h).ok())
                })
                .filter(|u| {
                    matches!(u.scheme(), "http" | "https" | "mailto")
                        && u.username().is_empty()
                        && u.password().is_none()
                })
        {
            let same_page = base.is_some_and(|base| {
                let mut page = base.clone();
                page.set_fragment(None);
                let mut target = href.clone();
                target.set_fragment(None);
                page == target
            });
            let href =
                if same_page && let Some(fragment) = href.fragment().filter(|f| valid_anchor(f)) {
                    format!("#{fragment}")
                } else {
                    href.to_string()
                };
            attrs.push(("href", href));
            attrs.push(("rel", "noreferrer noopener".into()));
        }
        if tag == "img" {
            let source = [
                "data-src",
                "data-original",
                "data-lazy-src",
                "data-url",
                "src",
            ]
            .iter()
            .find_map(|name| {
                element
                    .value()
                    .attr(name)
                    .filter(|v| !v.starts_with("data:") && !v.starts_with("blob:"))
            })
            .unwrap_or_default();
            let source = if source.starts_with("/api/media/")
                && source.trim_start_matches("/api/media/").len() == 36
            {
                source.to_owned()
            } else if remote {
                base.and_then(|base| base.join(source).ok())
                    .filter(|u| {
                        !source.is_empty()
                            && matches!(u.scheme(), "http" | "https")
                            && u.username().is_empty()
                            && u.password().is_none()
                    })
                    .map(|u| u.to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            if source.is_empty() {
                return;
            }
            attrs.push(("src", source));
            attrs.push((
                "alt",
                element.value().attr("alt").unwrap_or_default().into(),
            ));
        }
        if matches!(tag, "th" | "td") {
            for attr in ["colspan", "rowspan"] {
                if let Some(value) = element
                    .value()
                    .attr(attr)
                    .and_then(|v| v.parse::<u16>().ok())
                    .filter(|v| *v > 0 && *v <= 1000)
                {
                    attrs.push((attr, value.to_string()));
                }
            }
            if tag == "th"
                && let Some(scope) = element
                    .value()
                    .attr("scope")
                    .filter(|v| matches!(*v, "row" | "col" | "rowgroup" | "colgroup"))
            {
                attrs.push(("scope", scope.into()));
            }
        }
        if matches!(tag, "col" | "colgroup")
            && let Some(span) = element
                .value()
                .attr("span")
                .and_then(|v| v.parse::<u16>().ok())
                .filter(|v| *v > 0 && *v <= 1000)
        {
            attrs.push(("span", span.to_string()));
        }
        if allowed {
            out.push('<');
            out.push_str(tag);
            for (name, value) in attrs {
                out.push(' ');
                out.push_str(name);
                out.push_str("=\"");
                out.push_str(&escape(&value));
                out.push('"');
            }
            out.push('>');
        }
        for child in element.children() {
            if let Some(t) = child.value().as_text() {
                out.push_str(&escape(t));
            } else if let Some(e) = ElementRef::wrap(child) {
                emit(e, base, remote, out);
            }
        }
        if allowed && !matches!(tag, "br" | "hr" | "img" | "col") {
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
        }
    }
    let document = Html::parse_fragment(html);
    let base = Url::parse(base).ok();
    let mut out = String::new();
    emit(
        document.root_element(),
        base.as_ref(),
        remote_images,
        &mut out,
    );
    out
}
pub fn text(html: &str) -> String {
    normalized(&crate::rich_content::RichDocument::parse(html).plain_text())
}
pub fn word_count(html: &str) -> u32 {
    text(html)
        .split_whitespace()
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

/// Media players and social cards cannot run natively. Replace them with honest
/// static equivalents: a link to watch the video, or the post's published text.
fn embed_html(node: ElementRef<'_>, page_url: &Url) -> Option<String> {
    let element = node.value();
    let video = match element.name() {
        "video" => Some(page_url.to_string()),
        "iframe" => {
            let source = element.attr("src").and_then(|s| page_url.join(s).ok())?;
            let host = source.host_str().unwrap_or_default();
            let id = source
                .path_segments()
                .and_then(|mut s| s.next_back())
                .unwrap_or_default();
            if host.ends_with("youtube.com") || host.ends_with("youtube-nocookie.com") {
                Some(format!("https://www.youtube.com/watch?v={id}"))
            } else if host == "player.vimeo.com" {
                Some(format!("https://vimeo.com/{id}"))
            } else {
                // Unknown frames are not video evidence; keep the existing removal.
                return None;
            }
        }
        _ if element.attr("data-component-name") == Some("VideoEmbedPlayer") => {
            Some(page_url.to_string())
        }
        _ => None,
    };
    if let Some(link) = video {
        let target = if link.contains("youtube.com") {
            "Watch on YouTube"
        } else if link.contains("vimeo.com") {
            "Watch on Vimeo"
        } else {
            "Watch on the original page"
        };
        return Some(format!(
            "<figure><figcaption>Video · <a href=\"{}\">{target}</a></figcaption></figure>",
            escape(&link)
        ));
    }
    if element.attr("data-component-name") != Some("Twitter2ToDOM") {
        return None;
    }
    let attrs = std::iter::once(node)
        .chain(node.descendants().filter_map(ElementRef::wrap))
        .find_map(|e| e.value().attr("data-attrs"))?;
    let tweet: serde_json::Value = serde_json::from_str(attrs).ok()?;
    let field = |name: &str| tweet.get(name).and_then(|v| v.as_str()).unwrap_or_default();
    let text = Html::parse_fragment(field("full_text"))
        .root_element()
        .text()
        .collect::<String>();
    if text.trim().is_empty() {
        return None;
    }
    let mut out = String::from("<blockquote>");
    for paragraph in text.split("\n\n").filter(|p| !p.trim().is_empty()) {
        let lines: Vec<_> = paragraph.trim().lines().map(|l| escape(l.trim())).collect();
        out.push_str(&format!("<p>{}</p>", lines.join("<br>")));
    }
    let mut photos = String::new();
    for photo in tweet
        .get("photos")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter_map(|p| p.get("img_url")?.as_str())
    {
        photos.push_str(&format!("<p><img src=\"{}\" alt=\"\"></p>", escape(photo)));
    }
    let date = chrono::DateTime::parse_from_rfc3339(field("date"))
        .map(|d| d.format("%b %-d, %Y").to_string())
        .unwrap_or_default();
    let byline = match (field("name"), field("username")) {
        ("", "") => String::new(),
        (name, "") => escape(name),
        ("", user) => format!("@{}", escape(user)),
        (name, user) => format!("{} (@{})", escape(name), escape(user)),
    };
    let link = if date.is_empty() {
        "View post".to_owned()
    } else {
        date
    };
    let source = if field("url").is_empty() {
        link
    } else {
        format!("<a href=\"{}\">{link}</a>", escape(field("url")))
    };
    out.push_str(&format!(
        "<p>— {}{source}</p></blockquote>{photos}",
        if byline.is_empty() {
            String::new()
        } else {
            format!("{byline} · ")
        }
    ));
    Some(out)
}
pub fn extract(body: &str, url: &str) -> Result<Extraction, String> {
    if body.len() > 5 * 1024 * 1024 {
        return Err("Publisher response exceeds 5 MB".into());
    }
    let page_url = Url::parse(url).map_err(|e| e.to_string())?;
    if !matches!(page_url.scheme(), "http" | "https") {
        return Err("Publisher URL must be HTTP(S)".into());
    }
    let source = Html::parse_document(body);
    let source_root = source
        .select(&select("[itemprop=articleBody]"))
        .next()
        .or_else(|| source.select(&select("article")).next());
    let original_paragraphs: Vec<String> = source_root
        .map(|r| {
            r.select(&select("p"))
                .filter(|p| !excluded(*p) && !footnote(*p))
                .map(|p| normalized(&p.text().collect::<String>()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let mut declared_restricted = false;
    let mut declared_words = 0;
    for script in source.select(&select("script[type='application/ld+json']")) {
        if let Ok(v) = serde_json::from_str(&script.text().collect::<String>()) {
            json_evidence(&v, 0, &mut declared_restricted, &mut declared_words);
        }
    }
    let access_nodes = source
        .select(&select(
            "[class*='paywall'],[id*='paywall'],[class*='regwall'],[id*='regwall']",
        ))
        .any(|e| {
            !excluded(e)
                && !normalized(&e.text().collect::<String>()).is_empty()
                && e.value()
                    .attr("id")
                    .into_iter()
                    .chain(e.value().classes())
                    .any(|v| {
                        v.split(['-', '_'])
                            .any(|part| matches!(part, "paywall" | "regwall"))
                    })
        });
    let form_gate = source.select(&select("form")).any(|form| {
        let text = normalized(&form.text().collect::<String>());
        let lower = text.to_lowercase();
        restricted(&text)
            && ![
                "more posts",
                "future posts",
                "new posts",
                "newsletters",
                "more articles",
                "future articles",
                "new articles",
            ]
            .iter()
            .any(|v| lower.contains(v))
    });
    let source_text = source_root
        .map(|r| text(&clean_html(&r.inner_html(), url, true)))
        .unwrap_or_else(|| text(&clean_html(body, url, true)));
    // Protect structures at their selected original positions. Readability can remove
    // tables and footnotes on density heuristics; restoring only retained placeholders
    // avoids appending unrelated page structures or invented content.
    let source_sha256 = format!("{:x}", Sha256::digest(body.as_bytes()));
    let mut prepared = source.html();
    let mut structures = Vec::new();
    if let Some(root) = source_root {
        // Headings, rules and embeds are protected too: Readability drops headings
        // with "header" classes and empty rule wrappers on density heuristics.
        let nodes: Vec<_> = root
            .select(&select(
                "figure,img,table,details,.footnote,.footnotes,[role=doc-footnote],[role=doc-endnotes],h1,h2,h3,h4,h5,h6,hr,video,iframe,[data-component-name=VideoEmbedPlayer],[data-component-name=Twitter2ToDOM]",
            ))
            .filter(|n| {
                !excluded(*n) && (n.value().name() != "iframe" || embed_html(*n, &page_url).is_some())
            })
            .collect();
        let protected: std::collections::HashSet<_> = nodes.iter().map(|n| n.id()).collect();
        for (index, node) in nodes.into_iter().enumerate() {
            if node
                .ancestors()
                .filter_map(ElementRef::wrap)
                .take_while(|p| p.id() != root.id())
                .any(|p| protected.contains(&p.id()))
            {
                continue;
            }
            let marker = format!("reader-structure-{}-{index}", &source_sha256[..16]);
            let original = node.html();
            let replacement = embed_html(node, &page_url).unwrap_or_else(|| original.clone());
            let structure_text = normalized(
                &Html::parse_fragment(&replacement)
                    .root_element()
                    .text()
                    .collect::<String>(),
            );
            // Empty image-only structures need a nonempty placeholder, otherwise
            // Readability removes the blank paragraph. This private marker is
            // restored at its retained position and cannot reach saved prose.
            let sentinel = format!("READER_PRIVATE_STRUCTURE_{}_{}", source_sha256, index);
            let placeholder = format!(
                "<p id=\"{marker}\">{}</p>",
                escape(if structure_text.is_empty() {
                    &sentinel
                } else {
                    &structure_text
                })
            );
            prepared = prepared.replacen(&original, &placeholder, 1);
            // Losing an added layout structure is not missing article content.
            let essential = !matches!(
                node.value().name(),
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "hr" | "video" | "iframe"
            ) && node.value().attr("data-component-name").is_none();
            structures.push((marker, replacement, sentinel, essential));
        }
    }
    // Hidden/nonarticle controls and scripts remain data and cannot execute.
    let prepared_dom = Html::parse_document(&prepared);
    for hidden in prepared_dom.select(&select("[hidden],[aria-hidden=true],[style],form,.subscription-widget,.wp-block-jetpack-subscriptions")){if excluded(hidden)||hidden.value().name()=="form"||hidden.value().classes().any(|c|matches!(c,"subscription-widget"|"wp-block-jetpack-subscriptions")){prepared=prepared.replacen(&hidden.html(),"",1);}}
    let config = Config {
        max_elements_to_parse: 100_000,
        char_threshold: 150,
        ..Default::default()
    };
    let mut reader =
        Readability::new(prepared, Some(url), Some(config)).map_err(|e| e.to_string())?;
    let article = reader
        .parse()
        .map_err(|e| format!("No readable article found: {e}"))?;
    let mut content = article.content.to_string();
    let mut missing_structure = false;
    for (marker, original, sentinel, essential) in structures {
        let extracted = Html::parse_fragment(&content);
        if let Some(placeholder) = extracted
            .select(&Selector::parse(&format!("[id='{marker}']")).map_err(|e| e.to_string())?)
            .next()
        {
            content = content.replacen(&placeholder.html(), &original, 1);
        } else {
            missing_structure |= essential;
            // A parser may retain placeholder text while dropping its ID. Remove
            // the deterministic private text rather than inventing source prose.
            content = content.replace(&sentinel, "");
        }
    }
    let html = clean_html(&content, url, true);
    let word_count = word_count(&html);
    if word_count < 30 {
        return Err("The publisher returned too little article text.".into());
    }
    let text = text(&html);
    let mut reasons = Vec::new();
    if declared_restricted
        || access_nodes
        || form_gate
        || restricted(&source_text)
        || restricted(&text)
    {
        reasons.push("The publisher indicates restricted or blocked access.");
    }
    if word_count < 150 {
        reasons.push("Only a short article body was returned.");
    }
    let ending = text.trim_end_matches(['.', '!', '?', ' ']).to_lowercase();
    if [
        "this is a preview",
        "this is an excerpt",
        "continue reading",
        "read the full article",
        "read the rest of the article",
        "read full story",
        "…",
        "...",
    ]
    .iter()
    .any(|v| ending.ends_with(v))
    {
        reasons.push("The returned body appears to end in a preview or truncation.");
    }
    if declared_words as f64 > f64::from(word_count) * 1.25
        && declared_words.saturating_sub(u64::from(word_count)) > 30
    {
        reasons.push("The saved body is shorter than the publisher’s declared word count.");
    }
    let source_words = original_paragraphs.join(" ").split_whitespace().count();
    if source_words as f64 > f64::from(word_count) * 1.1
        && source_words.saturating_sub(word_count as usize) > 30
    {
        reasons.push("Some publisher article paragraphs were not retained.");
    }
    if original_paragraphs
        .last()
        .is_some_and(|p| !text.contains(p))
    {
        reasons.push("The publisher’s final paragraph was not retained.");
    }
    if missing_structure {
        reasons.push("Some article images or tables could not be retained.");
    }
    let mut logo_sources: Vec<String> = source
        .select(&select("link[rel~='icon'],link[rel='apple-touch-icon']"))
        .filter_map(|e| e.value().attr("href"))
        .filter_map(|href| page_url.join(href).ok())
        .filter(|u| {
            matches!(u.scheme(), "https" | "http")
                && u.username().is_empty()
                && u.password().is_none()
        })
        .map(|u| u.to_string())
        .collect();
    if let Ok(icon) = page_url.join("/favicon.ico") {
        logo_sources.push(icon.to_string());
    }
    logo_sources.dedup();
    Ok(Extraction {
        title: article.title,
        author: article.byline,
        site: article
            .site_name
            .unwrap_or_else(|| page_url.host_str().unwrap_or_default().to_owned()),
        html,
        word_count,
        excerpt: text.chars().take(260).collect(),
        status: if reasons.is_empty() {
            CopyStatus::Saved
        } else {
            CopyStatus::Partial
        },
        error: if reasons.is_empty() {
            None
        } else {
            Some(reasons.join(" ").chars().take(500).collect())
        },
        published_at: article.published_time,
        logo_sources,
        source_sha256,
    })
}
/// Require all incoming semantic text blocks, in order. A shared ending or word
/// count cannot verify a retained copy. Tables and footnotes participate equally.
pub fn contains_reading_copy(retained_html: &str, complete_html: &str) -> bool {
    fn visit(e: ElementRef<'_>, retained: &str, position: &mut usize, matched: &mut bool) -> bool {
        if matches!(
            e.value().name(),
            "p" | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "pre"
                | "li"
                | "figcaption"
                | "td"
                | "th"
                | "caption"
                | "summary"
        ) {
            let fragment = compact(&e.text().collect::<String>());
            if fragment.is_empty() {
                return true;
            }
            if let Some(index) = retained[*position..].find(&fragment) {
                *position += index + fragment.len();
                *matched = true;
                return true;
            }
            return false;
        }
        for node in e.children() {
            if let Some(text) = node.value().as_text() {
                let fragment = compact(text);
                if !fragment.is_empty() {
                    let Some(index) = retained[*position..].find(&fragment) else {
                        return false;
                    };
                    *position += index + fragment.len();
                    *matched = true;
                }
            } else if let Some(child) = ElementRef::wrap(node)
                && !visit(child, retained, position, matched)
            {
                return false;
            }
        }
        true
    }
    let retained = compact(
        &Html::parse_fragment(retained_html)
            .root_element()
            .text()
            .collect::<String>(),
    );
    let incoming = Html::parse_fragment(complete_html);
    let mut position = 0;
    let mut matched = false;
    visit(
        incoming.root_element(),
        &retained,
        &mut position,
        &mut matched,
    ) && matched
}
pub fn retention_decision(
    retained_html: &str,
    retained_status: CopyStatus,
    incoming: &Extraction,
) -> RetentionDecision {
    let retained_words = word_count(retained_html);
    if retained_words > 0
        && (incoming.word_count < retained_words
            || (retained_status == CopyStatus::Saved && incoming.status != CopyStatus::Saved))
    {
        RetentionDecision::Keep {
            verified: incoming.status == CopyStatus::Saved
                && contains_reading_copy(retained_html, &incoming.html),
            diagnostic: if incoming.word_count < retained_words {
                "The refreshed body was shorter; the existing saved copy was retained.".into()
            } else {
                "The refreshed copy was weaker; the existing saved copy was retained.".into()
            },
        }
    } else {
        RetentionDecision::Replace
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headings_rules_and_embeds_survive_readability() {
        let prose =
            "Reading copy keeps every sentence of this paragraph intact for the reader. ".repeat(6);
        let tweet = r#"{"url":"https://x.com/someone/status/1","full_text":"First line\n\nSecond <span class=\"tweet-fake-link\">@friend</span>","username":"someone","name":"Some One","date":"2026-10-21T04:20:59.000Z","photos":[]}"#;
        let html = format!(
            "<html><body><article><p>{prose}</p><div><hr/></div>\
             <h4 class=\"header-anchor-post\">(1) <a href=\"https://example.org/story\">Section title</a><div class=\"header-anchor-parent\"><button></button></div></h4>\
             <p>{prose}</p>\
             <div data-component-name=\"VideoEmbedPlayer\" id=\"media-1\"><div aria-hidden=\"true\"></div></div>\
             <p>{prose}</p>\
             <a href=\"https://x.com/someone/status/1\" data-component-name=\"Twitter2ToDOM\"><div class=\"twitter-embed\" data-attrs=\"{}\"><img src=\"https://example.org/avatar.jpg\"><p>Some One@someone</p></div></a>\
             <iframe src=\"https://www.youtube-nocookie.com/embed/abc123?rel=0\"></iframe>\
             <p>{prose}</p></article></body></html>",
            escape(tweet)
        );
        let extraction = extract(&html, "https://example.org/p/post").unwrap();
        let out = &extraction.html;
        assert!(out.contains("<hr>"), "{out}");
        assert!(
            out.contains("<h4>(1) <a href=\"https://example.org/story\""),
            "{out}"
        );
        assert!(
            out.contains("Video · <a href=\"https://example.org/p/post\""),
            "{out}"
        );
        assert!(
            out.contains("<a href=\"https://www.youtube.com/watch?v=abc123\""),
            "{out}"
        );
        assert!(
            out.contains("<blockquote><p>First line</p><p>Second @friend</p>"),
            "{out}"
        );
        assert!(
            out.contains("— Some One (@someone) · <a href=\"https://x.com/someone/status/1\""),
            "{out}"
        );
        assert!(!out.contains("avatar.jpg"), "{out}");
        assert!(!out.contains("READER_PRIVATE_STRUCTURE"), "{out}");
    }
    #[test]
    fn sanitizer_keeps_native_footnote_targets_and_same_page_links() {
        let html = clean_html(
            "<p>Text<a href='#note-1' onclick='bad()'><sup>1</sup></a><a href='https://example.org/article#note-1'>same page</a><a href='https://other.org/#note-1'>external</a></p><p id='note-1' onmouseover='bad()'>Footnote</p>",
            "https://example.org/article",
            false,
        );
        assert!(html.contains("id=\"note-1\""));
        assert_eq!(html.matches("href=\"#note-1\"").count(), 2);
        assert!(html.contains("href=\"https://other.org/#note-1\""));
        assert!(!html.contains("bad()"));
        let document = crate::rich_content::RichDocument::parse(&html);
        assert!(document.blocks.iter().any(|block| matches!(block,crate::rich_content::Block::Paragraph {anchor:Some(anchor),..} if anchor=="note-1")));
    }
    #[test]
    fn containment_requires_middle_table_and_order() {
        let retained = "<p>Opening</p><p>Middle</p><table><tr><td>Cell</td></tr></table><p>End</p>";
        assert!(contains_reading_copy(
            retained,
            "<p>Opening</p><table><tr><td>Cell</td></tr></table><p>End</p>"
        ));
        assert!(!contains_reading_copy(
            retained,
            "<p>Opening</p><p>Invented middle</p><p>End</p>"
        ));
        assert!(!contains_reading_copy(retained, "<p>End</p><p>Opening</p>"));
        assert!(!contains_reading_copy(retained, ""));
    }
    #[test]
    fn sanitization_preserves_structures_and_denies_execution() {
        let html = clean_html(
            "<script>bad()</script><p onclick='bad()'>Hello <strong>world</strong><a href='javascript:bad()'>link</a></p><table><tr><td colspan='2'>Source cell</td></tr></table><img src='https://example.org/a.png'><iframe src='https://evil.invalid'></iframe>",
            "https://example.org",
            false,
        );
        assert!(html.contains("<strong>world</strong>"));
        assert!(html.contains("colspan=\"2\""));
        assert!(!html.contains("bad()"));
        assert!(!html.contains("<img"));
    }
    #[test]
    fn uncaptained_figure_and_standalone_image_keep_source_positions() {
        let words = (0..90)
            .map(|n| format!("sourceword{n} "))
            .collect::<String>();
        let source = format!(
            "<html><head><title>Original article</title></head><body><article><h1>Original article</h1><p>Opening original paragraph. {words}</p><figure><img src='/first.png' alt='Original first image'></figure><p>Middle original paragraph. {words}</p><img src='/second.png' alt='Original second image'><p>Final original paragraph. {words}Final exact ending.</p></article></body></html>"
        );
        let copy = extract(&source, "https://example.org/story").unwrap();
        let dom = Html::parse_fragment(&copy.html);
        let images: Vec<_> = dom
            .select(&select("img"))
            .map(|image| image.value().attr("src").unwrap().to_owned())
            .collect();
        assert_eq!(
            images,
            vec![
                "https://example.org/first.png",
                "https://example.org/second.png"
            ]
        );
        let opening = copy.html.find("Opening original paragraph").unwrap();
        let first = copy.html.find("https://example.org/first.png").unwrap();
        let middle = copy.html.find("Middle original paragraph").unwrap();
        let second = copy.html.find("https://example.org/second.png").unwrap();
        let ending = copy.html.find("Final exact ending.").unwrap();
        assert!(opening < first && first < middle && middle < second && second < ending);
        assert!(
            !copy.html.contains("READER_PRIVATE_STRUCTURE_")
                && !copy.html.contains("reader-structure-")
        );
        assert_eq!(copy.status, CopyStatus::Saved);
    }
}
