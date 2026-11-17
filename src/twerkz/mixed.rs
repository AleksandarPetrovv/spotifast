//! Mixed playback: Soloist plays Spotify songs, and the built-in player
//! plays each stretch of local files between them (an interlude).

pub fn is_local(uri: &str) -> bool {
    uri.starts_with("spotify:local:")
}

/// A stretch of local files the built-in player plays as a plain list.
#[derive(Clone, Debug, PartialEq)]
pub struct Interlude {
    pub locals: Vec<String>,
    pub after: After,
}

/// What happens once the interlude's last local file ends.
#[derive(Clone, Debug, PartialEq)]
pub enum After {
    /// Soloist was paused on this song: seek it to 0 and resume it.
    ResumeSoloist { uri: String },
    /// Soloist has nothing loaded yet: start this context at this song on it.
    Start {
        context_uri: Option<String>,
        offset_uri: String,
    },
    /// Soloist is paused on the song the listener chose a local song above:
    /// move it on to the next one.
    NextSoloist,
    /// Nothing follows: stop.
    Nothing,
}

/// The consecutive local songs from `from` in `order`, and the first
/// non-local song after them.
pub fn local_run(order: &[String], from: usize) -> (Vec<String>, Option<String>) {
    let tail = order.get(from..).unwrap_or_default();
    let run: Vec<String> = tail
        .iter()
        .take_while(|u| is_local(u))
        .cloned()
        .collect();
    let next = tail.get(run.len()).filter(|_| !run.is_empty()).cloned();
    (run, next)
}

/// Like `local_run`, but a run that reaches the end of `order` goes on at its
/// start (repeat all). The song after the run is `None` when it is empty.
pub fn local_run_wrapping(order: &[String], from: usize) -> (Vec<String>, Option<String>) {
    let n = order.len();
    let run: Vec<String> = (0..n)
        .map(|step| &order[(from + step) % n])
        .take_while(|u| is_local(u))
        .cloned()
        .collect();
    let next = (!run.is_empty() && run.len() < n).then(|| order[(from + run.len()) % n].clone());
    (run, next)
}

/// What follows a local run that ends at `end` in `order`: the next Spotify
/// song and the list left to play. With `wrap`, a run that ends the list
/// goes on at its first Spotify song, with the whole list ahead.
pub fn run_after(order: &[String], end: usize, wrap: bool) -> (Option<String>, Vec<String>) {
    let rest = order.get(end..).unwrap_or_default();
    match rest.first() {
        Some(next) => (Some(next.clone()), rest.to_vec()),
        None if wrap => (
            order.iter().find(|u| !is_local(u)).cloned(),
            order.to_vec(),
        ),
        None => (None, Vec::new()),
    }
}

/// Previous at the start of an interlude that plays `locals` before the
/// Spotify song `resume`: the same interlude one local song earlier, or
/// `None` when no local song comes before it.
pub fn step_back(order: &[String], resume: &str, locals: &[String]) -> Option<Vec<String>> {
    let at = order.iter().position(|u| u == resume)?;
    let run = local_run_before(order, at);
    let start = run.len().checked_sub(locals.len()).filter(|s| *s > 0)?;
    (run[start..] == *locals).then(|| run[start - 1..].to_vec())
}

/// Previous at the start of an interlude that plays `locals`, when the song
/// played before them (`before`) is local too: the interlude with it first.
pub fn locals_with_before(before: Option<&str>, locals: &[String]) -> Option<Vec<String>> {
    let before = before.filter(|u| is_local(u))?;
    Some(std::iter::once(before.to_string()).chain(locals.iter().cloned()).collect())
}

/// The songs mixed playback has played in a context, oldest first, so
/// Previous can go back to a local song under shuffle too.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    context: String,
    played: Vec<String>,
}

impl History {
    const LIMIT: usize = 500;

    /// Starts over when the playing context is not this history's.
    pub fn follow(&mut self, context: &str) {
        if self.context != context {
            self.context = context.to_string();
            self.played.clear();
        }
    }

    pub fn clear(&mut self) {
        self.played.clear();
    }

    pub fn push(&mut self, uri: &str) {
        if self.played.last().map(String::as_str) == Some(uri) {
            return;
        }
        if self.played.len() == Self::LIMIT {
            self.played.remove(0);
        }
        self.played.push(uri.to_string());
    }

    /// A local run starts playing. When the tail of the history is already
    /// the start of this run (it is played again from further back or from
    /// its first song), that tail is replaced rather than repeated.
    pub fn enter(&mut self, run: &[String]) {
        if let Some(at) = run.first().and_then(|first| self.played.iter().rposition(|u| u == first)) {
            let tail = &self.played[at..];
            if run.len() >= tail.len() && run[..tail.len()] == *tail {
                self.played.truncate(at);
            }
        }
        run.iter().for_each(|uri| self.push(uri));
    }

    /// Goes back from the run that starts with `first`: forgets the run and
    /// the song before it, and returns that song.
    pub fn back_from(&mut self, first: &str) -> Option<String> {
        let at = self.played.iter().rposition(|u| u == first).filter(|at| *at > 0)?;
        self.played.truncate(at);
        self.played.pop()
    }

    /// The song played right before `current`, which must be the latest.
    pub fn before(&self, current: &str) -> Option<&str> {
        let [.., before, last] = self.played.as_slice() else {
            return None;
        };
        (last == current).then_some(before.as_str())
    }

    /// Goes back from `current` (the latest song): forgets it and returns the
    /// song before, which becomes the latest. `None` changes nothing.
    pub fn step_back(&mut self, current: &str) -> Option<String> {
        self.before(current)?;
        self.played.pop();
        self.played.last().cloned()
    }
}

/// The consecutive local songs right before index `at` in `order`.
pub fn local_run_before(order: &[String], at: usize) -> Vec<String> {
    let head = order.get(..at).unwrap_or(order);
    let start = head.iter().rposition(|u| !is_local(u)).map_or(0, |i| i + 1);
    head[start..].to_vec()
}

/// Soloist moved from `previous` to `current`. Returns the local songs it
/// skipped between them in `order` (the context's unshuffled row order).
pub fn skipped_locals(order: &[String], previous: &str, current: &str) -> Vec<String> {
    if is_local(current) {
        return order
            .iter()
            .position(|u| u == current)
            .map(|i| local_run(order, i).0)
            .unwrap_or_default();
    }
    let n = order.len();
    for (i, _) in order.iter().enumerate().filter(|(_, u)| *u == previous) {
        let mut skipped = Vec::new();
        for step in 1..=n {
            let uri = &order[(i + step) % n];
            if !is_local(uri) {
                if uri == current {
                    return skipped;
                }
                break;
            }
            skipped.push(uri.clone());
        }
    }
    Vec::new()
}

/// Whether a context can start at a chosen song (Spotify's play request only
/// takes an offset song inside playlists and albums).
pub fn starts_at_song(context: &str) -> bool {
    context.starts_with("spotify:playlist:") || context.starts_with("spotify:album:")
}

/// A silent Soloist skip that has reported the song it landed on.
#[derive(Debug, PartialEq)]
pub enum Arrival {
    /// Not the wanted song and skips remain: skip again, still silent.
    Hop,
    /// The wanted song (any song when none is named), or no skips are left.
    There,
}

pub fn arrival(want: Option<&str>, hops: usize, landed: &str) -> Arrival {
    if hops > 0 && want.is_some_and(|want| want != landed) {
        Arrival::Hop
    } else {
        Arrival::There
    }
}

/// The built-in player now reports `track` (`None` once it has nothing).
/// True when the interlude has finished and `after` should run.
pub fn interlude_over(interlude: &Interlude, track: Option<&str>, stopped: bool) -> bool {
    stopped || track.is_none_or(|t| !interlude.locals.iter().any(|l| l == t))
}

/// The songs a shuffled run of one context has played, so local songs get
/// their fair share of the shuffle Soloist does on its own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShuffleRun {
    context: String,
    played: std::collections::HashSet<String>,
}

impl ShuffleRun {
    /// Starts over when the playing context is not this run's.
    pub fn follow(&mut self, context: &str) {
        if self.context != context {
            self.context = context.to_string();
            self.played.clear();
        }
    }

    pub fn reset(&mut self) {
        self.played.clear();
    }

    pub fn mark_played(&mut self, uri: &str) {
        self.played.insert(uri.to_string());
    }

    /// Unplayed local songs and unplayed Spotify songs.
    pub fn counts(&self, locals: &[String], spotify_total: usize) -> (usize, usize) {
        let k = locals.iter().filter(|u| !self.played.contains(*u)).count();
        let played_spotify = self.played.iter().filter(|u| !is_local(u)).count();
        (k, spotify_total.saturating_sub(played_spotify))
    }

    /// A random unplayed local song with probability k / (k + n), else
    /// `None`. `roll` is in [0, 1).
    pub fn pick(&self, locals: &[String], spotify_total: usize, roll: f64) -> Option<String> {
        let (k, n) = self.counts(locals, spotify_total);
        if k == 0 || roll * ((k + n) as f64) >= k as f64 {
            return None;
        }
        let index = ((roll * (k + n) as f64) as usize).min(k - 1);
        locals
            .iter()
            .filter(|u| !self.played.contains(*u))
            .nth(index)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    const L1: &str = "spotify:local:a";
    const L2: &str = "spotify:local:b";

    #[test]
    fn run_from_local() {
        let order = v(&["s1", L1, L2, "s2", L1]);
        assert_eq!(local_run(&order, 1), (v(&[L1, L2]), Some("s2".into())));
        assert_eq!(local_run(&order, 4), (v(&[L1]), None));
    }

    #[test]
    fn run_non_local_or_out_of_range() {
        let order = v(&["s1", L1]);
        assert_eq!(local_run(&order, 0), (vec![], None));
        assert_eq!(local_run(&order, 9), (vec![], None));
    }

    #[test]
    fn run_before_index() {
        let order = v(&["s1", L1, L2, "s2"]);
        assert_eq!(local_run_before(&order, 3), v(&[L1, L2]));
        assert!(local_run_before(&order, 1).is_empty());
        assert!(local_run_before(&order, 0).is_empty());
        assert_eq!(local_run_before(&v(&[L1, "s1"]), 1), v(&[L1]));
    }

    #[test]
    fn wrapping_run_crosses_the_end() {
        let order = v(&[L2, "s1", "s2", L1]);
        assert_eq!(local_run_wrapping(&order, 3), (v(&[L1, L2]), Some("s1".into())));
        assert_eq!(local_run_wrapping(&order, 4), (v(&[L2]), Some("s1".into())));
        assert_eq!(local_run_wrapping(&order, 2), (vec![], None));
        assert_eq!(local_run_wrapping(&v(&["s1", L1]), 2), (vec![], None));
    }

    #[test]
    fn wrapping_run_single_song_and_all_local() {
        assert_eq!(local_run_wrapping(&v(&["s1", L1]), 1), (v(&[L1]), Some("s1".into())));
        assert_eq!(local_run_wrapping(&v(&[L1, L2]), 0), (v(&[L1, L2]), None));
        assert_eq!(local_run_wrapping(&[], 0), (vec![], None));
    }

    #[test]
    fn run_after_plain_and_wrapped() {
        let order = v(&["s1", L1, "s2", L2]);
        assert_eq!(run_after(&order, 2, true), (Some("s2".into()), v(&["s2", L2])));
        assert_eq!(run_after(&order, 4, false), (None, vec![]));
        assert_eq!(run_after(&order, 4, true), (Some("s1".into()), order.clone()));
        assert_eq!(run_after(&v(&[L1]), 1, true), (None, v(&[L1])));
    }

    #[test]
    fn step_back_one_local_at_a_time() {
        let order = v(&["s0", L1, L2, "s1"]);
        assert_eq!(step_back(&order, "s1", &v(&[L2])), Some(v(&[L1, L2])));
        assert_eq!(step_back(&order, "s1", &v(&[L1, L2])), None);
        assert_eq!(step_back(&order, "s9", &v(&[L2])), None);
        assert_eq!(step_back(&order, "s1", &v(&[L1])), None);
        assert_eq!(step_back(&v(&["s0", L1, "s1"]), "s1", &v(&[L1])), None);
    }

    #[test]
    fn history_steps_back_through_local_and_spotify_songs() {
        let mut h = History::default();
        for u in ["s1", L1, "s2", "s2", L2, "s3"] {
            h.push(u);
        }
        assert_eq!(h.before("s3"), Some(L2));
        assert_eq!(h.before("s2"), None);
        assert_eq!(h.step_back("s3"), Some(L2.into()));
        assert_eq!(h.step_back(L2), Some("s2".into()));
        assert_eq!(h.step_back("s2"), Some(L1.into()));
        h.push(L1);
        assert_eq!(h.step_back(L1), Some("s1".into()));
        assert_eq!(h.step_back("s1"), None);
        assert_eq!(h.before("s1"), None);
    }

    #[test]
    fn history_follows_its_context_and_is_bounded() {
        let mut h = History::default();
        h.follow("a");
        h.push("s1");
        h.push("s2");
        h.follow("a");
        assert_eq!(h.before("s2"), Some("s1"));
        h.follow("b");
        assert_eq!(h.before("s2"), None);
        for i in 0..600 {
            h.push(&format!("s{i}"));
        }
        assert_eq!(h.played.len(), History::LIMIT);
        assert_eq!(h.before("s599"), Some("s598"));
        h.clear();
        assert_eq!(h.before("s599"), None);
    }

    #[test]
    fn history_replays_and_goes_back_over_runs() {
        let mut h = History::default();
        h.push("s1");
        h.enter(&v(&[L1, L2]));
        h.push("s2");
        assert_eq!(h.step_back("s2"), Some(L2.into()));
        h.enter(&v(&[L2]));
        assert_eq!(h.before(L2), Some(L1));
        h.enter(&v(&[L1, L2]));
        assert_eq!(h.played, v(&["s1", L1, L2]));
        assert_eq!(h.back_from(L1), Some("s1".into()));
        assert!(h.played.is_empty());
        assert_eq!(h.back_from(L1), None);
    }

    #[test]
    fn local_before_joins_the_interlude() {
        let locals = v(&[L2]);
        assert_eq!(locals_with_before(Some(L1), &locals), Some(v(&[L1, L2])));
        assert_eq!(locals_with_before(Some("s1"), &locals), None);
        assert_eq!(locals_with_before(None, &locals), None);
    }

    #[test]
    fn skipped_between() {
        let order = v(&["s1", L1, L2, "s2"]);
        assert_eq!(skipped_locals(&order, "s1", "s2"), v(&[L1, L2]));
        assert!(skipped_locals(&order, "s1", "s1").is_empty());
    }

    #[test]
    fn skipped_user_jump_or_unknown() {
        let order = v(&["s1", L1, "s2", "s3"]);
        assert!(skipped_locals(&order, "s1", "s3").is_empty());
        assert!(skipped_locals(&order, "s2", "s1").is_empty());
        assert!(skipped_locals(&order, "nope", "s2").is_empty());
    }

    #[test]
    fn skipped_duplicates() {
        let order = v(&["s1", "s2", "s1", L1, "s3"]);
        assert_eq!(skipped_locals(&order, "s1", "s3"), v(&[L1]));
        assert!(skipped_locals(&order, "s1", "s2").is_empty());
    }

    #[test]
    fn skipped_wraparound() {
        let order = v(&[L2, "s1", "s2", L1]);
        assert_eq!(skipped_locals(&order, "s2", "s1"), v(&[L1, L2]));
    }

    #[test]
    fn skipped_current_local() {
        let order = v(&["s1", L1, L2, "s2"]);
        assert_eq!(skipped_locals(&order, "s1", L1), v(&[L1, L2]));
    }

    #[test]
    fn silent_skips_stop_on_the_wanted_song_or_when_out_of_skips() {
        assert_eq!(arrival(Some("s2"), 2, "s1"), Arrival::Hop);
        assert_eq!(arrival(Some("s2"), 2, "s2"), Arrival::There);
        assert_eq!(arrival(Some("s2"), 0, "s1"), Arrival::There);
        assert_eq!(arrival(None, 3, "s1"), Arrival::There);
    }

    #[test]
    fn only_playlists_and_albums_start_at_a_song() {
        assert!(starts_at_song("spotify:playlist:abc"));
        assert!(starts_at_song("spotify:album:abc"));
        assert!(!starts_at_song("spotify:user:me:collection"));
        assert!(!starts_at_song("spotify:artist:abc"));
    }

    #[test]
    fn interlude_end_conditions() {
        let i = Interlude {
            locals: v(&[L1, L2]),
            after: After::Nothing,
        };
        assert!(!interlude_over(&i, Some(L1), false));
        assert!(interlude_over(&i, Some("spotify:track:x"), false));
        assert!(interlude_over(&i, None, false));
        assert!(interlude_over(&i, Some(L1), true));
    }

    #[test]
    fn shuffle_pick_probability_edges() {
        let run = ShuffleRun::default();
        assert_eq!(run.pick(&[], 5, 0.0), None);
        assert_eq!(run.pick(&v(&[L1]), 0, 0.999), Some(L1.into()));
        assert_eq!(run.pick(&v(&[L1]), 3, 0.24), Some(L1.into()));
        assert_eq!(run.pick(&v(&[L1]), 3, 0.25), None);
        assert_eq!(run.pick(&[], 0, 0.0), None);
    }

    #[test]
    fn shuffle_pick_is_uniform_over_unplayed() {
        let run = ShuffleRun::default();
        let locals = v(&[L1, L2]);
        assert_eq!(run.pick(&locals, 0, 0.0), Some(L1.into()));
        assert_eq!(run.pick(&locals, 0, 0.99), Some(L2.into()));
    }

    #[test]
    fn shuffle_pick_never_repeats_and_counts_spotify() {
        let mut run = ShuffleRun::default();
        let locals = v(&[L1, L2]);
        run.mark_played(L1);
        assert_eq!(run.pick(&locals, 0, 0.0), Some(L2.into()));
        run.mark_played(L2);
        assert_eq!(run.pick(&locals, 0, 0.0), None);
        run.mark_played("spotify:track:x");
        assert_eq!(run.counts(&locals, 4), (0, 3));
    }

    #[test]
    fn shuffle_run_resets() {
        let mut run = ShuffleRun::default();
        run.follow("spotify:playlist:a");
        run.mark_played(L1);
        run.follow("spotify:playlist:a");
        assert_eq!(run.counts(&v(&[L1]), 1), (0, 1));
        run.follow("spotify:playlist:b");
        assert_eq!(run.counts(&v(&[L1]), 1), (1, 1));
        run.mark_played(L1);
        run.reset();
        assert_eq!(run.counts(&v(&[L1]), 1), (1, 1));
    }
}
