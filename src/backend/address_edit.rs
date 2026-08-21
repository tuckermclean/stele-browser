//! `AddressEdit` — the pure, CI-testable address-bar edit-buffer core.
//!
//! design: docs/superpowers/specs/2026-08-21-chrome-address-edit-design.md §1.
//! Plan: docs/superpowers/plans/2026-08-21-chrome-address-edit-plan.md, Task 1.
//!
//! A sibling pure module to `chrome.rs`/`x11.rs` — deliberately NOT folded
//! into `chrome.rs` (that module's own doc comment scopes it to "pure
//! layout + drawing, no event handling... no history/animation state").
//! `AddressEdit` IS edit state, the same category as `browser::History`,
//! just x11-shell-local. It never touches `fetch::Url`/`browser::History` —
//! `run_x11` seeds it from `history.current().as_str()` and resolves its
//! `commit()` output itself, keeping this a pure string/cursor machine that
//! is trivial to unit-test in isolation.
//!
//! `cursor` indexes by **char count, not byte offset** — URLs are
//! near-universally ASCII, but every operation here must stay total over
//! any `char`, including pasted/typed non-ASCII punctuation, so indexing by
//! chars (never raw byte slicing on a char-index) avoids ever landing
//! mid-UTF-8-codepoint. Every mutating method re-clamps `cursor` to
//! `0..=buffer.chars().count()` before doing anything else — defensive
//! against a future caller misusing the API, even though today's only
//! caller (`run_x11`) always stays within the invariant by construction.

/// Hard cap on `buffer`'s length (in chars) — a stuck key / key-repeat
/// storm must not grow the buffer unboundedly. Inserts past the cap are
/// silently dropped: still total, no panic, no unbounded reallocation.
pub const MAX_LEN: usize = 2048;

/// The address bar's live edit state: what's been typed, where the cursor
/// is, and whether the field is currently focused (editable) at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddressEdit {
    pub buffer: String,
    /// Char index into `buffer`, always in `0..=buffer.chars().count()`.
    pub cursor: usize,
    pub focused: bool,
}

impl AddressEdit {
    /// Enter edit mode, seeded with `seed` (the current, real navigated
    /// URL — the caller's job, this struct never touches `Url`/`History`).
    /// Cursor starts at the end (common convention; select-all-on-focus is
    /// a plausible alternative but adds a "selection range" concept this
    /// MVP doesn't need).
    pub fn focus(&mut self, seed: &str) {
        self.buffer = seed.to_string();
        self.cursor = self.buffer.chars().count();
        self.focused = true;
    }

    /// Leave edit mode. Deliberately does NOT restore/clear `buffer` —
    /// "no restore-on-cancel logic lives here": when `focused` is `false`,
    /// the caller renders the real current URL (`ChromeState.url`), not
    /// whatever is sitting in `buffer`, so there is nothing to remember.
    pub fn blur(&mut self) {
        self.focused = false;
    }

    /// Insert `c` at the cursor and advance the cursor by one. Filters to
    /// printable, non-control chars (`!c.is_control()`) — belt-and-suspenders
    /// totality guard against a hostile/misbehaving caller, not a live bug
    /// fix (today's only caller, `keysym_to_key`, never emits a control
    /// char via `X11Key::Char`). Silently drops the insert once `buffer` is
    /// already at [`MAX_LEN`] chars.
    pub fn insert_char(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        self.clamp_cursor();
        if self.buffer.chars().count() >= MAX_LEN {
            return;
        }
        let byte_idx = self.byte_index(self.cursor);
        self.buffer.insert(byte_idx, c);
        self.cursor += 1;
    }

    /// Remove the char immediately before the cursor and move the cursor
    /// back by one. No-op at `cursor == 0`.
    pub fn backspace(&mut self) {
        self.clamp_cursor();
        if self.cursor == 0 {
            return;
        }
        let remove_at = self.cursor - 1;
        let byte_idx = self.byte_index(remove_at);
        if let Some(ch) = self.buffer[byte_idx..].chars().next() {
            self.buffer.drain(byte_idx..byte_idx + ch.len_utf8());
            self.cursor -= 1;
        }
    }

    /// Remove the char immediately after the cursor; cursor unchanged.
    /// No-op at `cursor == buffer.chars().count()` (end of buffer).
    pub fn delete_forward(&mut self) {
        self.clamp_cursor();
        let len = self.buffer.chars().count();
        if self.cursor >= len {
            return;
        }
        let byte_idx = self.byte_index(self.cursor);
        if let Some(ch) = self.buffer[byte_idx..].chars().next() {
            self.buffer.drain(byte_idx..byte_idx + ch.len_utf8());
        }
    }

    /// Move the cursor one char left, clamped at `0`.
    pub fn move_left(&mut self) {
        self.clamp_cursor();
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Move the cursor one char right, clamped at `buffer.chars().count()`.
    pub fn move_right(&mut self) {
        self.clamp_cursor();
        let len = self.buffer.chars().count();
        self.cursor = (self.cursor + 1).min(len);
    }

    /// Move the cursor to the start of the buffer.
    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    /// Move the cursor to the end of the buffer.
    pub fn move_end(&mut self) {
        self.cursor = self.buffer.chars().count();
    }

    /// Trim `buffer`. If the trimmed result is non-empty, returns
    /// `Some(trimmed)` and blurs (`focused = false`) — Enter navigates and
    /// blurs, in one step. If the trimmed result is empty/whitespace-only,
    /// returns `None` and leaves `focused` UNCHANGED — Enter on an empty or
    /// blank address bar is a no-op, not an accidental blur-with-nothing-to
    /// navigate-to.
    pub fn commit(&mut self) -> Option<String> {
        let trimmed = self.buffer.trim();
        if trimmed.is_empty() {
            None
        } else {
            let out = trimmed.to_string();
            self.focused = false;
            Some(out)
        }
    }

    /// Re-clamp `cursor` to `0..=buffer.chars().count()` — called at the
    /// top of every mutating method that reads `cursor` before using it, so
    /// no method can ever index past the buffer's char count.
    fn clamp_cursor(&mut self) {
        let len = self.buffer.chars().count();
        if self.cursor > len {
            self.cursor = len;
        }
    }

    /// Byte offset of char index `char_idx` in `buffer` — `buffer.len()`
    /// (i.e. "append at the end") when `char_idx` is at or past the char
    /// count, never a panic/OOB.
    fn byte_index(&self, char_idx: usize) -> usize {
        self.buffer.char_indices().nth(char_idx).map(|(i, _)| i).unwrap_or(self.buffer.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------- focus

    #[test]
    fn focus_seeds_buffer_cursor_at_end_and_sets_focused() {
        let mut a = AddressEdit::default();
        a.focus("http://example.test/");
        assert_eq!(a.buffer, "http://example.test/");
        assert_eq!(a.cursor, "http://example.test/".chars().count());
        assert_eq!(a.cursor, 21);
        assert!(a.focused);
    }

    // --------------------------------------------------------- insert_char

    #[test]
    fn insert_char_at_start_middle_and_end() {
        let mut a = AddressEdit::default();
        a.focus("");
        for c in "abc".chars() {
            a.insert_char(c);
        }
        assert_eq!(a.buffer, "abc");
        assert_eq!(a.cursor, 3);

        // Insert at the start.
        a.cursor = 0;
        a.insert_char('X');
        assert_eq!(a.buffer, "Xabc");
        assert_eq!(a.cursor, 1);

        // Insert in the middle.
        a.cursor = 2;
        a.insert_char('Y');
        assert_eq!(a.buffer, "XaYbc");
        assert_eq!(a.cursor, 3);

        // Insert at the end.
        a.cursor = a.buffer.chars().count();
        a.insert_char('Z');
        assert_eq!(a.buffer, "XaYbcZ");
        assert_eq!(a.cursor, 6);
    }

    #[test]
    fn insert_char_run_builds_expected_string_byte_for_byte() {
        let mut a = AddressEdit::default();
        a.focus("");
        for c in "http://example.test/".chars() {
            a.insert_char(c);
        }
        assert_eq!(a.buffer.as_bytes(), b"http://example.test/");
    }

    // ----------------------------------------------------------- backspace

    #[test]
    fn backspace_at_cursor_zero_is_a_no_op() {
        let mut a = AddressEdit::default();
        a.focus("abc");
        a.cursor = 0;
        a.backspace();
        assert_eq!(a.buffer, "abc");
        assert_eq!(a.cursor, 0);
    }

    #[test]
    fn backspace_at_end_removes_last_char_and_decrements_cursor() {
        let mut a = AddressEdit::default();
        a.focus("abc");
        a.backspace();
        assert_eq!(a.buffer, "ab");
        assert_eq!(a.cursor, 2);
    }

    #[test]
    fn stray_backspaces_on_empty_buffer_stay_at_empty_zero() {
        let mut a = AddressEdit::default();
        a.focus("");
        for _ in 0..10 {
            a.backspace();
        }
        assert_eq!(a.buffer, "");
        assert_eq!(a.cursor, 0);
    }

    // ------------------------------------------------------- delete_forward

    #[test]
    fn delete_forward_at_end_is_a_no_op() {
        let mut a = AddressEdit::default();
        a.focus("abc");
        assert_eq!(a.cursor, 3);
        a.delete_forward();
        assert_eq!(a.buffer, "abc");
        assert_eq!(a.cursor, 3);
    }

    #[test]
    fn delete_forward_at_start_removes_first_char_cursor_unchanged() {
        let mut a = AddressEdit::default();
        a.focus("abc");
        a.cursor = 0;
        a.delete_forward();
        assert_eq!(a.buffer, "bc");
        assert_eq!(a.cursor, 0);
    }

    // ------------------------------------------------------- cursor moves

    #[test]
    fn move_left_clamps_at_zero() {
        let mut a = AddressEdit::default();
        a.focus("ab");
        a.cursor = 0;
        for _ in 0..5 {
            a.move_left();
        }
        assert_eq!(a.cursor, 0);
    }

    #[test]
    fn move_right_clamps_at_end() {
        let mut a = AddressEdit::default();
        a.focus("ab");
        for _ in 0..5 {
            a.move_right();
        }
        assert_eq!(a.cursor, 2);
    }

    #[test]
    fn move_home_and_move_end() {
        let mut a = AddressEdit::default();
        a.focus("abcdef");
        a.cursor = 3;
        a.move_home();
        assert_eq!(a.cursor, 0);
        a.move_end();
        assert_eq!(a.cursor, 6);
    }

    // ------------------------------------------------------------- commit

    #[test]
    fn commit_non_empty_returns_trimmed_and_blurs() {
        let mut a = AddressEdit::default();
        a.focus("  http://example.test/  ");
        let out = a.commit();
        assert_eq!(out, Some("http://example.test/".to_string()));
        assert!(!a.focused);
    }

    #[test]
    fn commit_empty_buffer_returns_none_and_leaves_focused_unchanged() {
        let mut a = AddressEdit::default();
        a.focus("");
        assert!(a.focused);
        assert_eq!(a.commit(), None);
        assert!(a.focused, "empty commit must not blur");
    }

    #[test]
    fn commit_whitespace_only_buffer_returns_none_and_leaves_focused_unchanged() {
        let mut a = AddressEdit::default();
        a.focus("   ");
        assert_eq!(a.commit(), None);
        assert!(a.focused);
    }

    #[test]
    fn commit_only_whitespace_after_trim_returns_none() {
        let mut a = AddressEdit::default();
        a.focus("\t\n  \t");
        assert_eq!(a.commit(), None);
        assert!(a.focused);
    }

    // -------------------------------------------------------------- blur

    #[test]
    fn blur_sets_focused_false_and_does_not_touch_buffer_or_cursor() {
        let mut a = AddressEdit::default();
        a.focus("http://example.test/");
        let (buf_before, cursor_before) = (a.buffer.clone(), a.cursor);
        a.blur();
        assert!(!a.focused);
        assert_eq!(a.buffer, buf_before);
        assert_eq!(a.cursor, cursor_before);
    }

    // ------------------------------------------------------- control chars

    #[test]
    fn insert_char_rejects_control_chars() {
        let mut a = AddressEdit::default();
        a.focus("ab");
        let before = a.buffer.clone();
        let cursor_before = a.cursor;
        a.insert_char('\u{7}'); // BEL, a control char
        a.insert_char('\n');
        a.insert_char('\t');
        assert_eq!(a.buffer, before);
        assert_eq!(a.cursor, cursor_before);
    }

    // ------------------------------------------------------------- MAX_LEN

    #[test]
    fn insert_char_past_max_len_is_silently_dropped() {
        let mut a = AddressEdit::default();
        a.focus("");
        for _ in 0..(MAX_LEN + 50) {
            a.insert_char('x');
        }
        assert_eq!(a.buffer.chars().count(), MAX_LEN);
        assert_eq!(a.cursor, MAX_LEN);
    }

    // --------------------------------------------------------- non-ASCII

    #[test]
    fn non_ascii_chars_round_trip_without_panicking_or_splitting_codepoints() {
        let mut a = AddressEdit::default();
        a.focus("");
        for c in "héllo€世界".chars() {
            a.insert_char(c);
            assert!(String::from_utf8(a.buffer.clone().into_bytes()).is_ok());
        }
        assert_eq!(a.buffer, "héllo€世界");

        a.move_home();
        assert!(String::from_utf8(a.buffer.clone().into_bytes()).is_ok());
        a.delete_forward();
        assert!(String::from_utf8(a.buffer.clone().into_bytes()).is_ok());
        assert_eq!(a.buffer, "éllo€世界");

        a.move_end();
        a.backspace();
        assert!(String::from_utf8(a.buffer.clone().into_bytes()).is_ok());
        assert_eq!(a.buffer, "éllo€世");
    }

    // ------------------------------------------------- out-of-bounds cursor

    #[test]
    fn a_hostile_out_of_bounds_cursor_is_clamped_before_the_next_mutation_not_a_panic() {
        // A future caller misusing the public fields directly (not going
        // through focus/insert/etc.) could leave `cursor` past the char
        // count -- every mutating method re-clamps at its own top, so the
        // very next call must recover rather than panic/OOB.
        let mut a = AddressEdit::default();
        a.focus("abc");
        a.cursor = 1000;
        a.insert_char('!'); // must not panic; clamps then appends at the end
        assert_eq!(a.buffer, "abc!");
        assert_eq!(a.cursor, 4);

        a.cursor = 1000;
        a.backspace(); // must not panic
        assert_eq!(a.buffer, "abc");

        a.cursor = 1000;
        a.delete_forward(); // clamped cursor == len -> no-op, not a panic
        assert_eq!(a.buffer, "abc");
    }

    // --------------------------------------------------------- totality/fuzz

    /// A tiny deterministic LCG — no new crate dependency (no `rand`), just
    /// enough pseudo-randomness to drive a long, varied op sequence
    /// reproducibly across runs/platforms.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            // Numerical Recipes constants.
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0
        }
        fn next_u32(&mut self) -> u32 {
            (self.next() >> 32) as u32
        }
    }

    #[test]
    fn fuzz_long_random_op_sequence_never_panics_and_keeps_cursor_in_bounds() {
        let mut a = AddressEdit::default();
        let mut rng = Lcg(0xC0FFEE);
        let alphabet: Vec<char> = "abc123 :/.é€世-_?#&".chars().collect();

        for _ in 0..500 {
            match rng.next_u32() % 10 {
                0 => a.focus("http://seed.example/"),
                1 => a.blur(),
                2 => {
                    let c = alphabet[(rng.next_u32() as usize) % alphabet.len()];
                    a.insert_char(c);
                }
                3 => a.backspace(),
                4 => a.delete_forward(),
                5 => a.move_left(),
                6 => a.move_right(),
                7 => a.move_home(),
                8 => a.move_end(),
                _ => {
                    a.commit();
                }
            }
            assert!(
                a.cursor <= a.buffer.chars().count(),
                "cursor {} exceeded char count {} after an op",
                a.cursor,
                a.buffer.chars().count()
            );
            assert!(String::from_utf8(a.buffer.clone().into_bytes()).is_ok(), "buffer is not valid UTF-8");
        }
    }
}
