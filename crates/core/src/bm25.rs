use std::collections::HashMap;

use crate::catalog::CatalogEntry;

/// BM25 full-text search index over the tool catalog.
pub struct BM25Index {
    entries: Vec<CatalogEntry>,
    /// token → list of (doc_id, term_frequency)
    inverted: HashMap<String, Vec<(usize, f32)>>,
    doc_lengths: Vec<usize>,
    avg_doc_len: f32,
    k1: f32,
    b: f32,
}

impl BM25Index {
    /// Build a BM25 index from catalog entries.
    pub fn build(entries: &[CatalogEntry]) -> Self {
        let k1 = 1.2_f32;
        let b = 0.75_f32;

        let mut inverted: HashMap<String, Vec<(usize, f32)>> = HashMap::new();
        let mut doc_lengths = Vec::with_capacity(entries.len());

        for (doc_id, entry) in entries.iter().enumerate() {
            let text = format!("{} {} {}", entry.server, entry.name, entry.description);
            let tokens = tokenize(&text);
            let doc_len = tokens.len();
            doc_lengths.push(doc_len);

            // Count term frequencies
            let mut tf_counts: HashMap<&str, usize> = HashMap::new();
            for token in &tokens {
                *tf_counts.entry(token.as_str()).or_default() += 1;
            }

            for (token, count) in tf_counts {
                inverted
                    .entry(token.to_string())
                    .or_default()
                    .push((doc_id, count as f32));
            }
        }

        let total_len: usize = doc_lengths.iter().sum();
        let avg_doc_len = if doc_lengths.is_empty() {
            0.0
        } else {
            total_len as f32 / doc_lengths.len() as f32
        };

        Self {
            entries: entries.to_vec(),
            inverted,
            doc_lengths,
            avg_doc_len,
            k1,
            b,
        }
    }

    /// Search the index and return the top-k results sorted by score descending.
    pub fn search<'a>(&'a self, query: &str, top_k: usize) -> Vec<(&'a CatalogEntry, f32)> {
        let n = self.entries.len() as f32;
        if n == 0.0 {
            return Vec::new();
        }

        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            // Return all entries with equal score when query is empty
            return self
                .entries
                .iter()
                .take(top_k)
                .map(|e| (e, 0.0_f32))
                .collect();
        }

        let mut scores = vec![0.0_f32; self.entries.len()];

        for token in &query_tokens {
            let Some(postings) = self.inverted.get(token.as_str()) else {
                continue;
            };

            let df = postings.len() as f32;
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();

            for &(doc_id, tf) in postings {
                let dl = self.doc_lengths[doc_id] as f32;
                let tf_norm = tf * (self.k1 + 1.0)
                    / (tf + self.k1 * (1.0 - self.b + self.b * dl / self.avg_doc_len));
                scores[doc_id] += idf * tf_norm;
            }
        }

        // Collect (entry, score) pairs with non-zero scores
        let mut results: Vec<(&CatalogEntry, f32)> = self
            .entries
            .iter()
            .zip(scores.iter())
            .filter(|&(_, score)| *score > 0.0)
            .map(|(entry, score)| (entry, *score))
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        results.truncate(top_k);
        results
    }
}

/// Tokenize text: lowercase, split on non-alphanumeric characters, filter empty tokens.
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(server: &str, name: &str, description: &str) -> CatalogEntry {
        CatalogEntry {
            server: server.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            input_schema: json!({}),
        }
    }

    #[test]
    fn test_bm25_search_finds_matching_tool() {
        let entries = vec![
            entry("github", "create_issue", "Create a new issue in a repository"),
            entry("slack", "send_message", "Send a message to a Slack channel"),
        ];
        let index = BM25Index::build(&entries);
        let results = index.search("github issue", 5);
        assert!(!results.is_empty(), "expected results");
        assert_eq!(results[0].0.name, "create_issue");
    }

    #[test]
    fn test_bm25_search_empty_query_returns_entries() {
        let entries = vec![
            entry("github", "create_issue", "Create a new issue"),
            entry("slack", "send_message", "Send a message"),
        ];
        let index = BM25Index::build(&entries);
        let results = index.search("", 10);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_bm25_search_no_match_returns_empty() {
        let entries = vec![entry("github", "create_issue", "Create a new issue")];
        let index = BM25Index::build(&entries);
        let results = index.search("xyzzy_nonexistent_term", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_bm25_search_top_k_limit() {
        let entries: Vec<_> = (0..10)
            .map(|i| entry("server", &format!("tool_{i}"), "generic tool description"))
            .collect();
        let index = BM25Index::build(&entries);
        let results = index.search("generic tool", 3);
        assert!(results.len() <= 3);
    }

    #[test]
    fn test_bm25_empty_index() {
        let index = BM25Index::build(&[]);
        let results = index.search("anything", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_bm25_scores_by_relevance() {
        let entries = vec![
            entry("github", "create_issue", "Create a new issue in a GitHub repository"),
            entry("github", "list_issues", "List all issues in a GitHub repository"),
            entry("slack", "send_message", "Send a message to a Slack channel"),
        ];
        let index = BM25Index::build(&entries);
        let results = index.search("create issue", 5);
        // Both github tools should rank above slack
        assert!(!results.is_empty());
        let top_name = results[0].0.name.as_str();
        assert!(
            top_name == "create_issue" || top_name == "list_issues",
            "expected a github tool, got: {top_name}"
        );
    }
}
