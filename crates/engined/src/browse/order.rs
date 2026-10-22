//! Search matching and sort order.

use super::{Entry, Kind, Sort, Tags};

/// Every word appears in the entry's name or tags (lower case).
pub(super) fn matches(words: &[String], e: &Entry, info: Option<&Tags>) -> bool {
    let mut hay = e.name.to_lowercase();
    if let Some(t) = info {
        for s in [&t.title, &t.artist, &t.album, &t.genre]
            .into_iter()
            .flatten()
        {
            hay.push(' ');
            hay.push_str(&s.to_lowercase());
        }
    }
    words.iter().all(|w| hay.contains(w.as_str()))
}

/// Folders first by name; tracks by `sort`, unknown values last, then by name.
pub(super) fn order(
    sort: Sort,
    a: &Entry,
    ia: Option<&Tags>,
    b: &Entry,
    ib: Option<&Tags>,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let by_name = || {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    };
    let folders = (a.kind == Kind::Track).cmp(&(b.kind == Kind::Track));
    if folders != Ordering::Equal || a.kind != Kind::Track {
        return folders.then_with(by_name);
    }
    // Some before None, so the unknown come last.
    fn known<T>(a: Option<T>, b: Option<T>, cmp: impl Fn(T, T) -> Ordering) -> Ordering {
        match (a, b) {
            (Some(a), Some(b)) => cmp(a, b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }
    let text = |t: Option<&Tags>, f: fn(&Tags) -> &Option<String>| {
        t.and_then(|t| f(t).as_ref()).map(|s| s.to_lowercase())
    };
    let title = |t: Option<&Tags>, e: &Entry| {
        text(t, |t| &t.title).unwrap_or_else(|| e.name.to_lowercase())
    };
    let num = |t: Option<&Tags>, f: fn(&Tags) -> Option<f64>| t.and_then(f);
    let ord = match sort {
        Sort::Order | Sort::Name => Ordering::Equal,
        Sort::Title => title(ia, a).cmp(&title(ib, b)),
        Sort::Artist => known(text(ia, |t| &t.artist), text(ib, |t| &t.artist), |x, y| {
            x.cmp(&y)
        })
        .then_with(|| title(ia, a).cmp(&title(ib, b))),
        Sort::Bpm => known(num(ia, |t| t.bpm), num(ib, |t| t.bpm), |x, y| {
            x.total_cmp(&y)
        }),
        Sort::Key => known(text(ia, |t| &t.key), text(ib, |t| &t.key), |x, y| x.cmp(&y)),
        Sort::Length => known(num(ia, |t| t.duration), num(ib, |t| t.duration), |x, y| {
            x.total_cmp(&y)
        }),
    };
    ord.then_with(by_name)
}
