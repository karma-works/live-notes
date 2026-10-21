//! Decision models: instead of text they return a probability per fixed answer
//! (TypeSafe's Jev and others on OpenRouter, OpenAI's Decisions API).
//!
//! - OpenRouter: `POST https://openrouter.ai/api/alpha/decisions` with `state` and named
//!   `questions`; a yes/no question is `{"type": "noul", "instructions", "criteria":
//!   {"true", "false"}}`, its answer `{"type": "noul", "noul": p}`.
//! - OpenAI: `POST /v1/decisions` with `input` and a `questions` array; a yes/no question
//!   is `{"type": "predicate", "name", "instructions"}`, its answer `{"name", "probability"}`.
//!
//! All questions go into one request; they are answered independently of each other.

use crate::client::check;
use crate::{Client, Error, Result, Usage};
use ln_types::ProviderKind;
use serde_json::{Map, Value, json};

/// A yes/no question about the state.
#[derive(Debug, Clone, PartialEq)]
pub struct YesNo {
    /// Unique key, e.g. `c0`.
    pub name: String,
    pub instructions: String,
    /// When the answer is yes.
    pub yes: String,
    /// When the answer is no.
    pub no: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decisions {
    /// Probability of "yes" per question name, in question order. Refused or missing
    /// answers count as 0.
    pub yes: Vec<(String, f32)>,
    pub usage: Usage,
}

impl Client {
    /// The decisions endpoint of this provider, if it has one.
    pub fn decisions_url(&self) -> Option<String> {
        let base = self.provider().base_url.trim_end_matches('/');
        match self.provider().kind {
            // `https://openrouter.ai/api/v1` → `https://openrouter.ai/api/alpha/decisions`
            ProviderKind::OpenRouter => Some(format!("{}/alpha/decisions", base.strip_suffix("/v1").unwrap_or(base))),
            ProviderKind::OpenAi => Some(format!("{base}/decisions")),
            _ => None,
        }
    }

    /// Asks all questions about `state` in one request.
    pub async fn decide_yes_no(&self, model: &str, state: &str, questions: &[YesNo]) -> Result<Decisions> {
        let url = self.decisions_url().ok_or_else(|| Error::Decode(format!("{} has no decisions API", self.provider().name)))?;
        let body = decisions_body(self.provider().kind, model, state, questions, self.deny_data_collection);
        let resp = self.req_url(reqwest::Method::POST, &url)?.json(&body).send().await?;
        let v = check(resp).await?;
        Ok(parse_decisions(&v, questions))
    }
}

pub(crate) fn decisions_body(kind: ProviderKind, model: &str, state: &str, questions: &[YesNo], deny_data_collection: bool) -> Value {
    if kind == ProviderKind::OpenAi {
        let qs: Vec<Value> = questions
            .iter()
            .map(|q| {
                json!({
                    "type": "predicate",
                    "name": q.name,
                    "instructions": format!("{}\nYes: {}\nNo: {}", q.instructions, q.yes, q.no),
                })
            })
            .collect();
        return json!({ "model": model, "input": state, "questions": qs });
    }
    let mut qs = Map::new();
    for q in questions {
        qs.insert(q.name.clone(), json!({ "type": "noul", "instructions": q.instructions, "criteria": { "true": q.yes, "false": q.no } }));
    }
    let mut body = json!({ "model": model, "state": state, "questions": qs });
    if deny_data_collection {
        body["provider"] = json!({ "data_collection": "deny" });
    }
    body
}

/// Reads both answer shapes: an object keyed by question name (OpenRouter) or an array of
/// answers with `name` (OpenAI).
pub(crate) fn parse_decisions(v: &Value, questions: &[YesNo]) -> Decisions {
    let prob = |a: &Value| -> f32 {
        if a["type"].as_str() == Some("refusal") {
            return 0.0;
        }
        ["noul", "probability", "predicate"].iter().find_map(|k| a[*k].as_f64()).unwrap_or(0.0).clamp(0.0, 1.0) as f32
    };
    let answers = &v["answers"];
    let yes = questions
        .iter()
        .map(|q| {
            let a = match answers {
                Value::Array(list) => list.iter().find(|a| a["name"].as_str() == Some(&q.name)).cloned().unwrap_or(Value::Null),
                other => other[&q.name].clone(),
            };
            (q.name.clone(), prob(&a))
        })
        .collect();
    let u = &v["usage"];
    let tokens = |keys: &[&str]| keys.iter().find_map(|k| u[*k].as_u64()).unwrap_or(0);
    let usage = Usage {
        prompt_tokens: tokens(&["input_tokens", "prompt_tokens"]),
        completion_tokens: tokens(&["output_tokens", "completion_tokens"]),
        cost: u["cost"].as_f64(),
    };
    Decisions { yes, usage }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ln_types::Provider;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn qs() -> Vec<YesNo> {
        vec![
            YesNo { name: "c0".into(), instructions: "Chinesisch-Kurs?".into(), yes: "ja".into(), no: "nein".into() },
            YesNo { name: "c1".into(), instructions: "Arztagent?".into(), yes: "ja".into(), no: "nein".into() },
        ]
    }

    #[test]
    fn urls_per_provider() {
        let or = Client::new(Provider::openrouter(), Some("k".into()));
        assert_eq!(or.decisions_url().unwrap(), "https://openrouter.ai/api/alpha/decisions");
        let oa = Client::new(Provider::openai(), Some("k".into()));
        assert_eq!(oa.decisions_url().unwrap(), "https://api.openai.com/v1/decisions");
        assert!(Client::new(Provider::ollama(), None).decisions_url().is_none());
    }

    #[test]
    fn bodies_and_answers() {
        let b = decisions_body(ProviderKind::OpenRouter, "typesafe/jev-1.14", "Notiz", &qs(), true);
        assert_eq!(b["questions"]["c1"]["type"], "noul");
        assert_eq!(b["questions"]["c0"]["criteria"]["true"], "ja");
        assert_eq!(b["provider"]["data_collection"], "deny");
        let b = decisions_body(ProviderKind::OpenAi, "gpt-6-luna", "Notiz", &qs(), true);
        assert_eq!(b["input"], "Notiz");
        assert_eq!(b["questions"][1]["name"], "c1");
        assert_eq!(b["questions"][1]["type"], "predicate");
        assert!(b.get("provider").is_none());

        let or = json!({"answers": {"c0": {"type": "noul", "noul": 0.91}, "c1": {"type": "refusal"}},
            "usage": {"input_tokens": 120, "output_tokens": 0, "cost": 0.000005}});
        let d = parse_decisions(&or, &qs());
        assert_eq!(d.yes, vec![("c0".into(), 0.91), ("c1".into(), 0.0)]);
        assert_eq!(d.usage.prompt_tokens, 120);
        assert_eq!(d.usage.cost, Some(0.000005));
        let oa = json!({"answers": [{"name": "c1", "type": "predicate", "probability": 0.7}], "usage": {"input_tokens": 80}});
        let d = parse_decisions(&oa, &qs());
        assert_eq!(d.yes, vec![("c0".into(), 0.0), ("c1".into(), 0.7)]);
    }

    #[tokio::test]
    async fn openrouter_request_goes_to_alpha_endpoint() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/alpha/decisions"))
            .and(header("authorization", "Bearer or-key"))
            .and(body_partial_json(json!({"model": "typesafe/jev-1.14", "state": "Vokabeln 你好"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "typesafe/jev-1.14",
                "answers": {"c0": {"type": "noul", "noul": 0.88}, "c1": {"type": "noul", "noul": 0.04}},
                "usage": {"input_tokens": 50, "output_tokens": 0, "cost": 0.0000021}
            })))
            .mount(&s)
            .await;
        let p = Provider { base_url: format!("{}/api/v1", s.uri()), ..Provider::openrouter() };
        let c = Client::new(p, Some("or-key".into()));
        let d = c.decide_yes_no("typesafe/jev-1.14", "Vokabeln 你好", &qs()).await.unwrap();
        assert_eq!(d.yes[0].1, 0.88);
        assert!(d.yes[1].1 < 0.1);

        let no_key = Client::new(Provider::openrouter(), None);
        assert!(matches!(no_key.decide_yes_no("m", "x", &qs()).await, Err(Error::MissingKey(_))));
    }
}
