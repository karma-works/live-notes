//! Prompts and response parsing for summary, live suggestions, answers and images.
//! Prompts are in English (models follow them best); the output language is set explicitly.

use crate::Message;
use serde::{Deserialize, Serialize};

fn lang_name(code: &str) -> &'static str {
    match code {
        "de" => "German",
        "en" => "English",
        "zh" => "Simplified Chinese",
        "fr" => "French",
        "es" => "Spanish",
        "it" => "Italian",
        "pt" => "Portuguese",
        "nl" => "Dutch",
        "ja" => "Japanese",
        "ko" => "Korean",
        _ => "the language of the conversation",
    }
}

// ---- summary ------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Task {
    pub text: String,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub due: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Summary {
    #[serde(default)]
    pub title: String,
    /// Markdown bullet list.
    #[serde(default)]
    pub summary: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    /// Additions to the user's own notes, shown as suggestions.
    #[serde(default)]
    pub note_additions: Vec<String>,
}

/// `channel_labels` are the device channel labels ("Ich", "Andere" …) that must not be
/// treated as people (same rule as Foresight).
pub fn summary(transcript: &str, my_notes: &str, out_lang: &str, channel_labels: &[&str]) -> Vec<Message> {
    let labels = channel_labels.join("\", \"");
    let sys = format!(
        "You turn meeting transcripts into concise notes. Write everything in {lang}.\n\
         Transcript lines look like `[HH:MM:SS] Speaker (lang): text`. The speaker labels \"{labels}\" are \
         audio channels (microphone / system audio), not names: never attribute statements to them; \
         use real names only if they are clearly given, otherwise describe points neutrally.\n\
         Respond with one JSON object and nothing else:\n\
         {{\"title\": short title (max 8 words),\n\
          \"summary\": [3-8 bullet strings],\n\
          \"decisions\": [strings, may be empty],\n\
          \"tasks\": [{{\"text\": string, \"assignee\": name or null, \"due\": string or null}}],\n\
          \"note_additions\": [facts from the transcript that complete the user's own notes; empty if none]}}\n\
         Be factual. Do not invent content. No filler.",
        lang = lang_name(out_lang)
    );
    let user =
        format!("User's own notes:\n{}\n\nTranscript:\n{}", if my_notes.trim().is_empty() { "(none)" } else { my_notes }, transcript);
    vec![Message::system(sys), Message::user(user)]
}

pub fn parse_summary(text: &str) -> Option<Summary> {
    serde_json::from_str(extract_json(text)?).ok()
}

impl Summary {
    pub fn summary_markdown(&self) -> String {
        self.summary.iter().map(|s| format!("- {}\n", s.trim_start_matches(['-', ' ']))).collect()
    }
    pub fn decisions_markdown(&self) -> String {
        self.decisions.iter().map(|s| format!("- {}\n", s.trim_start_matches(['-', ' ']))).collect()
    }
    pub fn tasks_markdown(&self) -> String {
        self.tasks
            .iter()
            .map(|t| {
                let mut s = format!("- [ ] {}", t.text.trim());
                match (&t.assignee, &t.due) {
                    (Some(a), Some(d)) => s.push_str(&format!(" — {a}, {d}")),
                    (Some(a), None) => s.push_str(&format!(" — {a}")),
                    (None, Some(d)) => s.push_str(&format!(" — {d}")),
                    _ => {}
                }
                s.push('\n');
                s
            })
            .collect()
    }
}

// ---- live suggestions (SPEC §5 F8) --------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuggestionQuery {
    /// Something in the notes/files could help right now.
    #[serde(default)]
    pub relevant: bool,
    #[serde(default, alias = "frage_an_mich")]
    pub question_to_me: bool,
    #[serde(default, alias = "suchanfrage")]
    pub query: String,
    #[serde(default, alias = "sprache")]
    pub lang: Option<String>,
}

pub fn suggestion_query(recent_transcript: &str) -> Vec<Message> {
    let sys = "You watch a live conversation and decide whether the user's own notes or files could help \
               right now (a question to the user, a reference to a document, figures, a past decision). \
               Respond with one JSON object: {\"relevant\": bool, \"question_to_me\": bool, \
               \"query\": search query in the language of the conversation (max 10 words), \"lang\": ISO 639-1}. \
               If nothing is worth looking up, set relevant to false and query to \"\".";
    vec![Message::system(sys), Message::user(format!("Last minute of the conversation:\n{recent_transcript}"))]
}

pub fn parse_suggestion_query(text: &str) -> Option<SuggestionQuery> {
    let q: SuggestionQuery = serde_json::from_str(extract_json(text)?).ok()?;
    Some(q)
}

/// A source excerpt given to the model; numbered from 1 in the prompt.
pub struct ContextChunk<'a> {
    pub title: &'a str,
    pub text: &'a str,
}

fn numbered(chunks: &[ContextChunk<'_>]) -> String {
    chunks.iter().enumerate().map(|(i, c)| format!("[{}] {}\n{}\n", i + 1, c.title, c.text.trim())).collect::<Vec<_>>().join("\n")
}

pub fn live_answer(query: &str, chunks: &[ContextChunk<'_>], out_lang: &str) -> Vec<Message> {
    let sys = format!(
        "Answer in at most 3 short sentences in {}. Use only the numbered sources and cite them like [1]. \
         If the sources do not answer it, say so in one sentence.",
        lang_name(out_lang)
    );
    vec![Message::system(sys), Message::user(format!("Sources:\n{}\nQuestion: {query}", numbered(chunks)))]
}

// ---- answers (SPEC §5 F6) -------------------------------------------------------------------

pub fn answer(question: &str, chunks: &[ContextChunk<'_>], history: &[(String, String)]) -> Vec<Message> {
    let sys = "You answer questions about the user's own notes and files. Answer in the language of the \
               question. Use only the numbered sources; cite them like [1] or [2][3]. If the sources do not \
               contain the answer, say that plainly. Be concise; use Markdown lists where helpful.";
    let mut msgs = vec![Message::system(sys)];
    for (q, a) in history {
        msgs.push(Message::user(q.clone()));
        msgs.push(Message::assistant(a.clone()));
    }
    msgs.push(Message::user(format!("Sources:\n{}\nQuestion: {question}", numbered(chunks))));
    msgs
}

// ---- images ---------------------------------------------------------------------------------

pub fn image_description(out_lang: &str) -> String {
    format!(
        "Describe this image for a search index in {}: what it shows, all readable text, numbers and names. \
         Plain text, max 120 words.",
        lang_name(out_lang)
    )
}

/// Finds the first JSON object in a model answer (handles ```json fences and prose around it).
pub fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let bytes = text.as_bytes();
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            match (esc, b) {
                (true, _) => esc = false,
                (false, b'\\') => esc = true,
                (false, b'"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_json() {
        let t = "Sure:\n```json\n{\"title\":\"Budget {Q1}\",\"summary\":[\"A\"],\"tasks\":[{\"text\":\"Send\",\"assignee\":\"Li\"}]}\n```";
        let s = parse_summary(t).unwrap();
        assert_eq!(s.title, "Budget {Q1}");
        assert_eq!(s.tasks_markdown(), "- [ ] Send — Li\n");
        assert_eq!(s.summary_markdown(), "- A\n");
    }

    #[test]
    fn suggestion_query_accepts_german_keys() {
        let q =
            parse_suggestion_query("{\"relevant\":true,\"frage_an_mich\":true,\"suchanfrage\":\"Angebot Li\",\"sprache\":\"de\"}").unwrap();
        assert!(q.question_to_me);
        assert_eq!(q.query, "Angebot Li");
    }

    #[test]
    fn prompts_mention_channel_rule() {
        let m = summary("[00:00:01] Ich (de): Hallo", "", "de", &["Ich", "Andere"]);
        match &m[0].content {
            crate::Content::Text(t) => {
                assert!(t.contains("German"));
                assert!(t.contains("\"Ich\", \"Andere\""));
            }
            _ => panic!(),
        }
    }
}
