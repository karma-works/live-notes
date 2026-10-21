use super::*;

fn chunk(t: &str) -> NewChunk {
    NewChunk { text: t.into(), lang: None, locator: Locator::default() }
}

fn emb(seed: f32) -> Vec<f32> {
    (0..768).map(|i| ((i as f32) * 0.37 + seed).sin()).collect()
}

fn add(ix: &mut Index, src: i64, path: &str, title: &str, chunks: &[NewChunk]) -> i64 {
    ix.upsert_file(&FileUpsert {
        source_id: src,
        path: Path::new(path),
        mime: "text/markdown",
        size: 10,
        mtime: 100,
        hash: b"h",
        title,
        lang: None,
        parser_version: 1,
        chunks,
    })
    .unwrap()
}

#[test]
fn fts_cjk_and_vector_search() {
    let mut ix = Index::open_in_memory().unwrap();
    let src = ix.upsert_source(Path::new("/notes"), SourceKind::Notes).unwrap();
    add(&mut ix, src, "/notes/a.md", "Budget", &[chunk("Das Budget für Q1 wird nächste Woche besprochen.")]);
    add(&mut ix, src, "/notes/b.md", "会议", &[chunk("我们下周再讨论预算。"), chunk("报价周五前发给你")]);
    add(&mut ix, src, "/notes/c.md", "Einkauf", &[chunk("Milch und Brot kaufen")]);

    let hits = ix.search("budget", None, &SearchFilter::default(), 5).unwrap();
    assert_eq!(hits[0].path, Path::new("/notes/a.md"));

    let hits = ix.search("讨论预算", None, &SearchFilter::default(), 5).unwrap();
    assert_eq!(hits[0].path, Path::new("/notes/b.md"));
    let hits = ix.search("预算", None, &SearchFilter::default(), 5).unwrap();
    assert_eq!(hits[0].path, Path::new("/notes/b.md"), "two-character CJK query");

    // Embed every chunk; the query vector equals chunk "Milch".
    let todo = ix.chunks_to_embed("m", 100).unwrap();
    assert_eq!(todo.len(), 4);
    assert!(todo.iter().any(|(_, t)| t.starts_with("title: Einkauf | text: Milch")));
    for (i, (id, _)) in todo.iter().enumerate() {
        ix.set_embedding(*id, "m", &emb(i as f32)).unwrap();
    }
    assert_eq!(ix.stats().unwrap().embedded, 4);
    let milk_idx = todo.iter().position(|(_, t)| t.contains("Milch")).unwrap();
    let hits = ix.search("zzz", Some(&emb(milk_idx as f32)), &SearchFilter::default(), 2).unwrap();
    assert_eq!(hits[0].path, Path::new("/notes/c.md"));
    assert!(hits[0].similarity.unwrap() > 0.99, "identical vector: {:?}", hits[0].similarity);
    assert!(hits[1].similarity.unwrap() < hits[0].similarity.unwrap());
    assert!(ix.chunks_to_embed("m", 10).unwrap().is_empty());
    assert_eq!(ix.chunks_to_embed("other", 10).unwrap().len(), 4);
}

#[test]
fn replace_remove_and_filters() {
    let mut ix = Index::open_in_memory().unwrap();
    let s1 = ix.upsert_source(Path::new("/a"), SourceKind::Folder).unwrap();
    let s2 = ix.upsert_source(Path::new("/b"), SourceKind::Folder).unwrap();
    add(&mut ix, s1, "/a/x.md", "X", &[chunk("alpha beta")]);
    add(&mut ix, s2, "/b/y.md", "Y", &[chunk("alpha gamma")]);
    // Re-index x with new content: old text must disappear from FTS.
    add(&mut ix, s1, "/a/x.md", "X", &[chunk("delta")]);
    assert!(ix.search("beta", None, &SearchFilter::default(), 5).unwrap().is_empty());
    let only_b = SearchFilter { source_ids: Some(vec![s2]), ..Default::default() };
    assert_eq!(ix.search("alpha", None, &only_b, 5).unwrap().len(), 1);
    let excl = SearchFilter { exclude_paths: vec!["/b/y.md".into()], ..Default::default() };
    assert!(ix.search("alpha", None, &excl, 5).unwrap().is_empty());

    let srcs = ix.sources().unwrap();
    assert_eq!(srcs.len(), 2);
    ix.remove_source(s2).unwrap();
    assert!(ix.search("gamma", None, &SearchFilter::default(), 5).unwrap().is_empty());
    ix.remove_file(Path::new("/a/x.md")).unwrap();
    assert_eq!(ix.stats().unwrap().files, 0);
}

#[test]
fn persists_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("i.sqlite");
    {
        let mut ix = Index::open(&p).unwrap();
        let s = ix.upsert_source(Path::new("/n"), SourceKind::Notes).unwrap();
        add(&mut ix, s, "/n/a.md", "A", &[chunk("persistent text")]);
    }
    let ix = Index::open(&p).unwrap();
    assert_eq!(ix.search("persistent", None, &SearchFilter::default(), 3).unwrap().len(), 1);
    assert!(ix.file(Path::new("/n/a.md")).unwrap().is_some());
}

#[test]
fn quantize_needs_256_dims() {
    assert!(quantize(&[0.1; 10]).is_err());
    let q = quantize(&vec![1.0; 300]).unwrap();
    assert_eq!(q.len(), 256);
}
