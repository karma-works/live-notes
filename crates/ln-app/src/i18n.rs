//! UI strings from `locales/*.ftl` (simple `key = value` messages, Fluent syntax).
//! No string in the UI is hard-coded (SPEC §16).

use ln_types::UiLanguage;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::LazyLock;

static DE: &str = include_str!("../locales/de.ftl");
static EN: &str = include_str!("../locales/en.ftl");

fn parse(src: &'static str) -> HashMap<&'static str, &'static str> {
    src.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once(" = "))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect()
}

static TABLES: LazyLock<[HashMap<&'static str, &'static str>; 2]> = LazyLock::new(|| [parse(DE), parse(EN)]);
static CURRENT: RwLock<UiLanguage> = RwLock::new(UiLanguage::En);

pub fn set_language(ui: UiLanguage) {
    *CURRENT.write() = ui;
}

pub fn language() -> UiLanguage {
    *CURRENT.read()
}

/// The message for `key` in the current UI language (English as fallback, then the key).
pub fn t(key: &str) -> &'static str {
    let i = match language() {
        UiLanguage::De => 0,
        UiLanguage::En => 1,
    };
    let tables = &*TABLES;
    tables[i].get(key).or_else(|| tables[1].get(key)).copied().unwrap_or_else(|| Box::leak(key.to_string().into_boxed_str()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn both_locales_have_the_same_keys() {
        let de = super::parse(super::DE);
        let en = super::parse(super::EN);
        let mut a: Vec<_> = de.keys().collect();
        let mut b: Vec<_> = en.keys().collect();
        a.sort();
        b.sort();
        assert_eq!(a, b);
    }
}
